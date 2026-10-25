//! Minimal Marionette client: the protocol Firefox exposes for remote control.
//! Messages are `<length>:<json>`; a command is `[0, id, name, params]` and the answer
//! `[1, id, error|null, result]`.

use std::fmt;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use base64::Engine;
use serde_json::{Value, json};

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    /// Firefox answered with an error (`error` code and message).
    Remote(String, String),
    Protocol(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "E/S: {e}"),
            Error::Remote(code, msg) => write!(f, "Firefox: {code}: {msg}"),
            Error::Protocol(m) => write!(f, "protocolo: {m}"),
        }
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Error {
        Error::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub struct Marionette {
    stream: TcpStream,
    buf: Vec<u8>,
    next_id: u64,
}

impl Marionette {
    /// Connect to `port`, retrying until `wait` passes (Firefox takes a moment to listen),
    /// read the greeting and open a session.
    pub fn connect(port: u16, wait: Duration) -> Result<Marionette> {
        let end = Instant::now() + wait;
        let stream = loop {
            match TcpStream::connect(("127.0.0.1", port)) {
                Ok(s) => break s,
                Err(e) if Instant::now() >= end => return Err(e.into()),
                Err(_) => std::thread::sleep(Duration::from_millis(150)),
            }
        };
        stream.set_read_timeout(Some(Duration::from_secs(60)))?;
        let mut m = Marionette {
            stream,
            buf: Vec::new(),
            next_id: 0,
        };
        m.read_message()?; // greeting: {"applicationType":"gecko","marionetteProtocol":3}
        m.call("WebDriver:NewSession", json!({ "capabilities": {} }))?;
        Ok(m)
    }

    fn read_message(&mut self) -> Result<Value> {
        let colon = loop {
            if let Some(i) = self.buf.iter().position(|&b| b == b':') {
                break i;
            }
            self.fill()?;
        };
        let len: usize = std::str::from_utf8(&self.buf[..colon])
            .ok()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| Error::Protocol("longitud de mensaje no válida".into()))?;
        while self.buf.len() < colon + 1 + len {
            self.fill()?;
        }
        let body: Vec<u8> = self.buf.drain(..colon + 1 + len).skip(colon + 1).collect();
        serde_json::from_slice(&body).map_err(|e| Error::Protocol(format!("JSON: {e}")))
    }

    fn fill(&mut self) -> Result<()> {
        let mut chunk = [0u8; 65536];
        let n = self.stream.read(&mut chunk)?;
        if n == 0 {
            return Err(Error::Protocol("Firefox cerró la conexión".into()));
        }
        self.buf.extend_from_slice(&chunk[..n]);
        Ok(())
    }

    /// Send a command and wait for its answer. A JavaScript dialog left open by the page
    /// is dismissed and the command retried once.
    pub fn call(&mut self, name: &str, params: Value) -> Result<Value> {
        match self.call_once(name, params.clone()) {
            Err(Error::Remote(code, _)) if code == "unexpected alert open" => {
                let _ = self.call_once("WebDriver:DismissAlert", json!({}));
                self.call_once(name, params)
            }
            other => other,
        }
    }

    fn call_once(&mut self, name: &str, params: Value) -> Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        let body = serde_json::to_vec(&json!([0, id, name, params])).expect("json");
        self.stream
            .write_all(format!("{}:", body.len()).as_bytes())?;
        self.stream.write_all(&body)?;
        loop {
            let msg = self.read_message()?;
            let Some(arr) = msg.as_array() else { continue };
            if arr.len() < 4 || arr[0] != 1 || arr[1] != id {
                continue;
            }
            return match &arr[2] {
                Value::Null => Ok(arr[3].clone()),
                err => Err(Error::Remote(
                    err["error"].as_str().unwrap_or("error").to_string(),
                    err["message"].as_str().unwrap_or("").to_string(),
                )),
            };
        }
    }

    // ---- WebDriver commands used by the browser ------------------------------------

    pub fn navigate(&mut self, url: &str) -> Result<()> {
        self.call("WebDriver:Navigate", json!({ "url": url }))
            .map(drop)
    }

    pub fn back(&mut self) -> Result<()> {
        self.call("WebDriver:Back", json!({})).map(drop)
    }

    pub fn forward(&mut self) -> Result<()> {
        self.call("WebDriver:Forward", json!({})).map(drop)
    }

    pub fn refresh(&mut self) -> Result<()> {
        self.call("WebDriver:Refresh", json!({})).map(drop)
    }

    /// Handles of every open window or tab.
    pub fn window_handles(&mut self) -> Result<Vec<String>> {
        let r = self.call("WebDriver:GetWindowHandles", json!({}))?;
        Ok(r.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|h| h.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default())
    }

    pub fn switch_to_window(&mut self, handle: &str) -> Result<()> {
        self.call("WebDriver:SwitchToWindow", json!({ "handle": handle }))
            .map(drop)
    }

    pub fn set_timeouts(&mut self, page_load_ms: u64) -> Result<()> {
        self.call(
            "WebDriver:SetTimeouts",
            json!({ "pageLoad": page_load_ms, "script": 20000 }),
        )
        .map(drop)
    }

    pub fn set_window_size(&mut self, width: u32, height: u32) -> Result<()> {
        self.call(
            "WebDriver:SetWindowRect",
            json!({ "x": 0, "y": 0, "width": width, "height": height }),
        )
        .map(drop)
    }

    /// Run `script` (a function body that may `return`) and give back its value.
    pub fn execute(&mut self, script: &str, args: Value) -> Result<Value> {
        let r = self.call(
            "WebDriver:ExecuteScript",
            json!({ "script": script, "args": args }),
        )?;
        Ok(r["value"].clone())
    }

    /// PNG of the visible area.
    pub fn screenshot(&mut self) -> Result<Vec<u8>> {
        let r = self.call("WebDriver:TakeScreenshot", json!({ "full": false }))?;
        let b64 = r["value"]
            .as_str()
            .ok_or_else(|| Error::Protocol("captura sin datos".into()))?;
        base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| Error::Protocol(format!("base64: {e}")))
    }

    pub fn perform_actions(&mut self, actions: Value) -> Result<()> {
        self.call("WebDriver:PerformActions", json!({ "actions": actions }))
            .map(drop)
    }

    pub fn release_actions(&mut self) -> Result<()> {
        self.call("WebDriver:ReleaseActions", json!({})).map(drop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn send(s: &mut TcpStream, v: Value) {
        let body = serde_json::to_vec(&v).unwrap();
        s.write_all(format!("{}:", body.len()).as_bytes()).unwrap();
        s.write_all(&body).unwrap();
    }

    fn recv(s: &mut TcpStream) -> Value {
        let mut len = String::new();
        let mut b = [0u8; 1];
        loop {
            s.read_exact(&mut b).unwrap();
            if b[0] == b':' {
                break;
            }
            len.push(b[0] as char);
        }
        let mut body = vec![0u8; len.parse().unwrap()];
        s.read_exact(&mut body).unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    /// A fake Firefox that answers a scripted conversation.
    fn fake(script: impl FnOnce(&mut TcpStream) + Send + 'static) -> u16 {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            send(
                &mut s,
                json!({"applicationType":"gecko","marionetteProtocol":3}),
            );
            let hello = recv(&mut s);
            assert_eq!(hello[2], "WebDriver:NewSession");
            send(&mut s, json!([1, hello[1], null, {"sessionId":"x"}]));
            script(&mut s);
        });
        port
    }

    #[test]
    fn opens_a_session_and_runs_commands() {
        let port = fake(|s| {
            let c = recv(s);
            assert_eq!(c[2], "WebDriver:ExecuteScript");
            assert_eq!(c[3]["script"], "return 1+1");
            // An unrelated message first: it must be skipped.
            send(s, json!([1, 999, null, {}]));
            send(s, json!([1, c[1], null, {"value": 2}]));
            let c = recv(s);
            send(s, json!([1, c[1], null, {"value": "aGk="}]));
            let c = recv(s);
            send(
                s,
                json!([1, c[1], {"error":"timeout","message":"too slow"}, null]),
            );
        });
        let mut m = Marionette::connect(port, Duration::from_secs(2)).unwrap();
        assert_eq!(m.execute("return 1+1", json!([])).unwrap(), json!(2));
        assert_eq!(m.screenshot().unwrap(), b"hi");
        match m.navigate("https://x.org") {
            Err(Error::Remote(code, msg)) => {
                assert_eq!((code.as_str(), msg.as_str()), ("timeout", "too slow"))
            }
            other => panic!("esperaba un error remoto: {other:?}"),
        }
    }

    #[test]
    fn an_open_dialog_is_dismissed_and_the_command_retried() {
        let port = fake(|s| {
            let c = recv(s);
            send(
                s,
                json!([1, c[1], {"error":"unexpected alert open","message":"alert"}, null]),
            );
            let d = recv(s);
            assert_eq!(d[2], "WebDriver:DismissAlert");
            send(s, json!([1, d[1], null, {}]));
            let again = recv(s);
            assert_eq!(again[2], "WebDriver:Navigate");
            assert_eq!(again[3]["url"], "https://x.org");
            send(s, json!([1, again[1], null, {}]));
        });
        let mut m = Marionette::connect(port, Duration::from_secs(2)).unwrap();
        m.navigate("https://x.org").unwrap();
    }

    #[test]
    fn handles_messages_split_across_reads() {
        let port = fake(|s| {
            let c = recv(s);
            let body = serde_json::to_vec(&json!([1, c[1], null, {"value": "x".repeat(200_000)}]))
                .unwrap();
            let framed = [format!("{}:", body.len()).as_bytes(), &body].concat();
            for chunk in framed.chunks(7000) {
                s.write_all(chunk).unwrap();
                s.flush().unwrap();
            }
        });
        let mut m = Marionette::connect(port, Duration::from_secs(2)).unwrap();
        let v = m.execute("x", json!([])).unwrap();
        assert_eq!(v.as_str().unwrap().len(), 200_000);
    }

    #[test]
    fn reports_a_closed_connection() {
        let port = fake(|s| {
            let _ = recv(s);
            // drop: close without answering
        });
        let mut m = Marionette::connect(port, Duration::from_secs(2)).unwrap();
        assert!(matches!(m.refresh(), Err(Error::Protocol(_))));
    }

    #[test]
    fn connect_gives_up_after_the_wait() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        let t = Instant::now();
        assert!(Marionette::connect(port, Duration::from_millis(400)).is_err());
        assert!(t.elapsed() < Duration::from_secs(3));
    }
}
