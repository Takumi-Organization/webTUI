//! webTUI — a browser for the terminal. See the README for the keys.

use std::io::{self, stdout};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event as Term, KeyEventKind,
};
use ratatui::crossterm::execute;
use webtui::app::{App, Mode};
use webtui::browser::Browser;
use webtui::url::normalize;

const USAGE: &str = "Uso: webtui [URL | dominio | archivo.html]\n\nNavegador para el terminal: Firefox como motor y celdas de color como pantalla.\nSin argumentos pide una dirección. Dentro, ? muestra las teclas.";

fn start_url(arg: &str) -> String {
    match Path::new(arg).canonicalize() {
        Ok(p) if p.is_file() => format!("file://{}", p.display()),
        _ => normalize(arg),
    }
}

fn main() -> io::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{USAGE}");
        return Ok(());
    }
    if args.iter().any(|a| a == "-V" || a == "--version") {
        println!("webtui {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let url = args.first().map(|a| start_url(a));

    let mut terminal = ratatui::init();
    let _ = execute!(stdout(), EnableMouseCapture);
    let result = run(&mut terminal, url);
    let _ = execute!(stdout(), DisableMouseCapture);
    ratatui::restore();
    result
}

/// When the terminal goes away crossterm's `poll` can spin forever and never return to the
/// loop, so the main thread cannot be trusted to notice the signal: this thread does.
/// It tells the worker to stop (killing Firefox and removing its profile) and then exits.
fn spawn_watchdog(stop: Arc<AtomicBool>, worker: std::sync::mpsc::Sender<webtui::browser::Cmd>) {
    std::thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = worker.send(webtui::browser::Cmd::Quit);
        std::thread::sleep(Duration::from_millis(1500));
        std::process::exit(129);
    });
}

fn run(terminal: &mut ratatui::DefaultTerminal, url: Option<String>) -> io::Result<()> {
    let size = terminal.size()?;
    let mut app = App::new(size.width, size.height.saturating_sub(2));
    if url.is_none() {
        app.mode = Mode::Url(String::new());
        app.status.clear();
    }
    // A closed terminal (SIGHUP) or a plain `kill` must still stop Firefox and remove its profile.
    let stop = Arc::new(AtomicBool::new(false));
    for sig in [
        signal_hook::consts::SIGHUP,
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGINT,
    ] {
        signal_hook::flag::register(sig, Arc::clone(&stop))?;
    }
    let mut browser = Browser::start(app.cols, app.rows, url);
    spawn_watchdog(Arc::clone(&stop), browser.sender());
    while !app.quit && !stop.load(Ordering::Relaxed) {
        while let Ok(ev) = browser.rx.try_recv() {
            app.on_event(ev);
        }
        if app.dirty {
            app.dirty = false;
            terminal.draw(|f| app.draw(f))?;
        }
        if event::poll(Duration::from_millis(40))? {
            let cmds = match event::read()? {
                Term::Key(k) if k.kind == KeyEventKind::Press => app.on_key(k),
                Term::Mouse(m) => app.on_mouse(m),
                Term::Resize(w, h) => app.resize(w, h).into_iter().collect(),
                _ => vec![],
            };
            for c in cmds {
                browser.send(c);
            }
        }
    }
    browser.shutdown();
    Ok(())
}
