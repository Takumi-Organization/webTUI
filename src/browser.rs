//! The browser worker: a thread that owns Firefox and answers commands with snapshots.
//! It also watches the page and sends a fresh snapshot when something changed on its own
//! (timers, scripts, animations that touch the DOM, late images), which is what makes
//! dynamic pages work.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::firefox::Firefox;
use crate::marionette::{Error, Marionette};
use crate::page::{COLLECT_JS, HIDE_TEXT_JS, SHOW_TEXT_JS, Snapshot};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Enter,
    Backspace,
    Delete,
    Tab,
    Escape,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
}

impl Key {
    /// WebDriver code point of the key.
    fn code(self) -> &'static str {
        match self {
            Key::Enter => "\u{e007}",
            Key::Backspace => "\u{e003}",
            Key::Delete => "\u{e017}",
            Key::Tab => "\u{e004}",
            Key::Escape => "\u{e00c}",
            Key::Left => "\u{e012}",
            Key::Up => "\u{e013}",
            Key::Right => "\u{e014}",
            Key::Down => "\u{e015}",
            Key::Home => "\u{e011}",
            Key::End => "\u{e010}",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Cmd {
    Goto(String),
    /// Scroll by this many CSS pixels (negative: up).
    Scroll(i32),
    ScrollTop,
    ScrollBottom,
    /// Click at a point of the viewport, in CSS pixels.
    Click(f32, f32),
    Type(String),
    Key(Key),
    Back,
    Forward,
    Reload,
    /// The page area is now this many cells.
    Resize(u16, u16),
    /// Pixels of page per terminal column (zoom).
    CellWidth(u32),
    Quit,
}

pub enum Event {
    Snapshot(Box<Snapshot>),
    Status(String),
    Error(String),
    /// Firefox is gone; the browser cannot go on.
    Fatal(String),
}

pub struct Browser {
    tx: Sender<Cmd>,
    pub rx: Receiver<Event>,
    handle: Option<JoinHandle<()>>,
}

impl Browser {
    pub fn start(cols: u16, rows: u16, url: Option<String>) -> Browser {
        let (tx, cmd_rx) = mpsc::channel();
        let (ev_tx, rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            if let Err(e) = worker(cmd_rx, &ev_tx, cols, rows, url) {
                let _ = ev_tx.send(Event::Fatal(e.to_string()));
            }
        });
        Browser {
            tx,
            rx,
            handle: Some(handle),
        }
    }

    pub fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }

    /// A second way to reach the worker (the signal watchdog uses it).
    pub fn sender(&self) -> Sender<Cmd> {
        self.tx.clone()
    }

    /// Ask the worker to stop and wait for Firefox to be gone.
    pub fn shutdown(&mut self) {
        let _ = self.tx.send(Cmd::Quit);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// ---- action payloads (pure, tested) ---------------------------------------------------

pub fn click_actions(x: f32, y: f32) -> Value {
    json!([{
        "type": "pointer", "id": "mouse", "parameters": { "pointerType": "mouse" },
        "actions": [
            { "type": "pointerMove", "x": x.round() as i64, "y": y.round() as i64, "origin": "viewport" },
            { "type": "pointerDown", "button": 0 },
            { "type": "pointerUp", "button": 0 },
        ]
    }])
}

pub fn key_actions(keys: &[String]) -> Value {
    let mut acts = Vec::new();
    for k in keys {
        acts.push(json!({ "type": "keyDown", "value": k }));
        acts.push(json!({ "type": "keyUp", "value": k }));
    }
    json!([{ "type": "key", "id": "keyboard", "actions": acts }])
}

// ---- the worker -------------------------------------------------------------------------

const POLL_JS: &str = r#"
if (!window.__webtui) {
  window.__webtui = { n: 0 };
  new MutationObserver(m => { window.__webtui.n += m.length; })
    .observe(document, { subtree: true, childList: true, attributes: true, characterData: true });
}
return [window.__webtui.n, scrollX, scrollY, innerWidth, innerHeight, location.href, document.readyState, document.title];
"#;

struct State {
    cols: u16,
    rows: u16,
    cell_w: u32,
    calibrated: bool,
    /// Poll result at the last snapshot: if it changes, the page changed.
    seen: Option<Value>,
    last_capture: Instant,
}

impl State {
    fn viewport(&self) -> (u32, u32) {
        (
            self.cols as u32 * self.cell_w,
            self.rows as u32 * self.cell_w * 2,
        )
    }
}

fn worker(
    rx: Receiver<Cmd>,
    tx: &Sender<Event>,
    cols: u16,
    rows: u16,
    url: Option<String>,
) -> Result<(), Error> {
    let _ = tx.send(Event::Status("Iniciando Firefox…".into()));
    let ff = Firefox::launch()?;
    let mut m = Marionette::connect(ff.port, Duration::from_secs(40))?;
    m.set_timeouts(30_000)?;
    let mut st = State {
        cols,
        rows,
        cell_w: 8,
        calibrated: false,
        seen: None,
        last_capture: Instant::now(),
    };
    resize(&mut m, &st)?;
    if let Some(u) = url {
        go(&mut m, tx, &u);
    }
    publish(&mut m, tx, &mut st);

    loop {
        match rx.recv_timeout(Duration::from_millis(400)) {
            Ok(Cmd::Quit) | Err(RecvTimeoutError::Disconnected) => break,
            Ok(cmd) => {
                let layout = matches!(cmd, Cmd::Resize(..) | Cmd::CellWidth(_));
                if let Err(e) = apply(&mut m, tx, &mut st, cmd) {
                    if matches!(e, Error::Io(_) | Error::Protocol(_)) {
                        return Err(e);
                    }
                    let _ = tx.send(Event::Error(e.to_string()));
                }
                if layout {
                    st.calibrated = true;
                }
                settle(&mut m);
                publish(&mut m, tx, &mut st);
            }
            Err(RecvTimeoutError::Timeout) => {
                // Something changed on its own? (not more than twice a second)
                if st.last_capture.elapsed() >= Duration::from_millis(450)
                    && let Ok(now) = poll(&mut m)
                    && st.seen.as_ref() != Some(&now)
                {
                    publish(&mut m, tx, &mut st);
                }
            }
        }
    }
    Ok(())
}

fn apply(m: &mut Marionette, tx: &Sender<Event>, st: &mut State, cmd: Cmd) -> Result<(), Error> {
    match cmd {
        Cmd::Goto(u) => go(m, tx, &u),
        Cmd::Scroll(dy) => {
            m.execute(
                "window.scrollBy({ top: arguments[0], left: 0, behavior: 'instant' });",
                json!([dy]),
            )?;
        }
        Cmd::ScrollTop => {
            m.execute(
                "window.scrollTo({ top: 0, behavior: 'instant' });",
                json!([]),
            )?;
        }
        Cmd::ScrollBottom => {
            m.execute(
                "window.scrollTo({ top: document.documentElement.scrollHeight, behavior: 'instant' });",
                json!([]),
            )?;
        }
        Cmd::Click(x, y) => {
            let before = m.window_handles().unwrap_or_default();
            m.perform_actions(click_actions(x, y))?;
            m.release_actions()?;
            follow_new_window(m, st, &before);
        }
        Cmd::Type(text) => {
            let keys: Vec<String> = text.chars().map(String::from).collect();
            m.perform_actions(key_actions(&keys))?;
            m.release_actions()?;
        }
        Cmd::Key(k) => {
            let before = m.window_handles().unwrap_or_default();
            m.perform_actions(key_actions(&[k.code().to_string()]))?;
            m.release_actions()?;
            follow_new_window(m, st, &before);
        }
        Cmd::Back => {
            let _ = m.back();
        }
        Cmd::Forward => {
            let _ = m.forward();
        }
        Cmd::Reload => m.refresh()?,
        Cmd::Resize(cols, rows) => {
            st.cols = cols.max(1);
            st.rows = rows.max(1);
            resize(m, st)?;
        }
        Cmd::CellWidth(w) => {
            st.cell_w = w.clamp(4, 14);
            resize(m, st)?;
        }
        Cmd::Quit => {}
    }
    Ok(())
}

/// A click or key may open a new tab (`target="_blank"`, `window.open`): show that one.
fn follow_new_window(m: &mut Marionette, st: &State, before: &[String]) {
    std::thread::sleep(Duration::from_millis(350));
    let Ok(now) = m.window_handles() else { return };
    if let Some(new) = now.iter().find(|h| !before.contains(h))
        && m.switch_to_window(new).is_ok()
    {
        let _ = resize(m, st);
    }
}

/// Navigate; a slow or failing load is reported but the page is shown anyway.
fn go(m: &mut Marionette, tx: &Sender<Event>, url: &str) {
    let _ = tx.send(Event::Status(format!("Cargando {url}…")));
    match m.navigate(url) {
        Ok(()) => {}
        Err(Error::Remote(code, msg)) if code == "timeout" || code == "unknown error" => {
            let _ = tx.send(Event::Error(format!("{code}: {msg}")));
        }
        Err(e) => {
            let _ = tx.send(Event::Error(e.to_string()));
        }
    }
}

/// Make the viewport exactly the size of the cell grid (Firefox adds window chrome).
fn resize(m: &mut Marionette, st: &State) -> Result<(), Error> {
    let (w, h) = st.viewport();
    m.set_window_size(w, h + 120)?;
    let inner = m.execute("return [innerWidth, innerHeight];", json!([]))?;
    let (iw, ih) = (
        inner[0].as_i64().unwrap_or(w as i64),
        inner[1].as_i64().unwrap_or(h as i64),
    );
    if (iw, ih) != (w as i64, h as i64) {
        let nw = (w as i64 + (w as i64 - iw)).max(1) as u32;
        let nh = (h as i64 + 120 + (h as i64 - ih)).max(1) as u32;
        m.set_window_size(nw, nh)?;
    }
    Ok(())
}

fn poll(m: &mut Marionette) -> Result<Value, Error> {
    m.execute(POLL_JS, json!([]))
}

/// Wait (briefly) until the page stops loading or changing after an action.
fn settle(m: &mut Marionette) {
    std::thread::sleep(Duration::from_millis(120));
    let mut prev: Option<Value> = None;
    for _ in 0..40 {
        match poll(m) {
            Ok(now) => {
                let complete = now[6] == "complete";
                if complete && prev.as_ref() == Some(&now) {
                    return;
                }
                prev = Some(now);
            }
            Err(Error::Io(_) | Error::Protocol(_)) => return,
            Err(_) => prev = None, // mid-navigation: no document to ask yet
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn capture(m: &mut Marionette) -> Result<Snapshot, Error> {
    let data = m.execute(COLLECT_JS, json!([]))?;
    m.execute(HIDE_TEXT_JS, json!([]))?;
    let png = m.screenshot();
    let _ = m.execute(SHOW_TEXT_JS, json!([]));
    Snapshot::from_json(&data, &png?).map_err(Error::Protocol)
}

/// Take a snapshot and send it; on the first content page also fit the cell width to the
/// font the page really uses and take it again.
fn publish(m: &mut Marionette, tx: &Sender<Event>, st: &mut State) {
    let mut snap = match capture(m) {
        Ok(s) => s,
        Err(e) => {
            let _ = tx.send(Event::Error(e.to_string()));
            return;
        }
    };
    if !st.calibrated
        && let Some(median) = snap.median_char_width()
    {
        st.calibrated = true;
        // Rounded down: a cell never wider than a letter, so text does not stretch.
        let w = (median.floor() as u32).clamp(6, 10);
        if w != st.cell_w {
            st.cell_w = w;
            if resize(m, st).is_ok() {
                settle(m);
                if let Ok(s) = capture(m) {
                    snap = s;
                }
            }
        }
    }
    // Our own style changes are not page changes: remember the state after them.
    st.seen = poll(m).ok();
    st.last_capture = Instant::now();
    let _ = tx.send(Event::Snapshot(Box::new(snap)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn click_is_a_move_then_press_and_release() {
        let a = click_actions(10.4, 20.6);
        let acts = &a[0]["actions"];
        assert_eq!(acts[0]["type"], "pointerMove");
        assert_eq!(
            (acts[0]["x"].as_i64(), acts[0]["y"].as_i64()),
            (Some(10), Some(21))
        );
        assert_eq!(acts[0]["origin"], "viewport");
        assert_eq!(
            (acts[1]["type"].as_str(), acts[2]["type"].as_str()),
            (Some("pointerDown"), Some("pointerUp"))
        );
    }

    #[test]
    fn typing_presses_and_releases_every_key() {
        let a = key_actions(&["h".into(), Key::Enter.code().into()]);
        let acts = a[0]["actions"].as_array().unwrap();
        assert_eq!(acts.len(), 4);
        assert_eq!(
            (acts[0]["type"].as_str(), acts[0]["value"].as_str()),
            (Some("keyDown"), Some("h"))
        );
        assert_eq!(acts[3]["value"], "\u{e007}");
    }

    #[test]
    fn the_viewport_is_the_cell_grid_in_pixels() {
        let st = State {
            cols: 100,
            rows: 30,
            cell_w: 8,
            calibrated: false,
            seen: None,
            last_capture: Instant::now(),
        };
        assert_eq!(st.viewport(), (800, 480));
    }
}
