//! End to end against a real headless Firefox and local pages. Skipped if Firefox is missing.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use webtui::browser::{Browser, Cmd, Event};
use webtui::page::{Kind, Snapshot};
use webtui::render::render;

const INDEX: &str = r#"<!doctype html><meta charset="utf-8"><title>Prueba</title>
<body style="margin:0;font:16px sans-serif">
<div style="background:#e00000;height:96px;color:#fff;padding:8px">Cabecera roja</div>
<div style="background:#0040e0;height:96px"></div>
<p style="padding:8px">Hola mundo <a href="segunda.html">ir a la segunda</a></p>
<input id="q" placeholder="buscar" style="margin:8px;width:300px;height:28px">
<p style="position:absolute;top:320px;left:8px;margin:0">tapado</p>
<div style="position:absolute;top:310px;left:0;width:300px;height:40px;background:#0a8a0a"></div>
<p style="position:absolute;top:400px;left:8px;margin:0">visible-sobre-nada</p>
<a href="segunda.html" style="position:absolute;clip:rect(0 0 0 0);width:1px;height:1px;overflow:hidden">oculto</a>
<div style="height:3000px"></div></body>"#;

const SECOND: &str = r#"<!doctype html><meta charset="utf-8"><title>Segunda</title>
<body style="margin:0;font:16px sans-serif"><h1>Segunda pagina</h1><a href="index.html">volver</a></body>"#;

const POPUP: &str = r#"<!doctype html><meta charset="utf-8"><title>Origen</title>
<body style="margin:0;font:16px sans-serif"><p><a href="segunda.html" target="_blank">abrir en otra pestaña</a></p></body>"#;

const DYNAMIC: &str = r#"<!doctype html><meta charset="utf-8"><title>Dinamica</title>
<body style="margin:0;font:16px sans-serif"><p id="t">antes</p><p id="n">cuenta 0</p>
<script>
setTimeout(() => { document.getElementById('t').textContent = 'despues'; }, 1500);
let i = 0; setInterval(() => { document.getElementById('n').textContent = 'cuenta ' + (++i); }, 700);
</script></body>"#;

fn firefox_available() -> bool {
    Command::new("firefox")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn pages() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("e2e-pages");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("index.html"), INDEX).unwrap();
    fs::write(dir.join("segunda.html"), SECOND).unwrap();
    fs::write(dir.join("dinamica.html"), DYNAMIC).unwrap();
    fs::write(dir.join("popup.html"), POPUP).unwrap();
    dir
}

/// Wait for a snapshot that satisfies `pred`; fail with what was seen otherwise.
fn wait_for(b: &Browser, what: &str, pred: impl Fn(&Snapshot) -> bool) -> Box<Snapshot> {
    let end = Instant::now() + Duration::from_secs(25);
    let mut seen = Vec::new();
    while Instant::now() < end {
        match b.rx.recv_timeout(Duration::from_millis(200)) {
            Ok(Event::Snapshot(s)) => {
                if pred(&s) {
                    return s;
                }
                seen.push(format!(
                    "{} sy={} [{}]",
                    s.url,
                    s.scroll_y,
                    s.words
                        .iter()
                        .take(8)
                        .map(|w| w.text.as_str())
                        .collect::<Vec<_>>()
                        .join(" ")
                ));
            }
            Ok(Event::Fatal(e)) => panic!("{what}: Firefox falló: {e}"),
            Ok(Event::Error(e)) => seen.push(format!("error: {e}")),
            Ok(Event::Status(_)) | Err(_) => {}
        }
    }
    panic!("{what}: no llegó la instantánea esperada. Vistas: {seen:#?}");
}

/// The number after the word "cuenta" in the dynamic page.
fn counter(s: &Snapshot) -> Option<u32> {
    let i = s.words.iter().position(|w| w.text == "cuenta")?;
    s.words.get(i + 1)?.text.parse().ok()
}

fn has_word(s: &Snapshot, w: &str) -> bool {
    s.words.iter().any(|x| x.text == w)
}

#[test]
fn the_engine_renders_scrolls_clicks_types_and_follows_dynamic_pages() {
    if !firefox_available() {
        eprintln!("Firefox no está instalado: prueba omitida");
        return;
    }
    let dir = pages();
    let url = |f: &str| format!("file://{}/{f}", dir.display());
    let mut b = Browser::start(100, 30, Some(url("index.html")));

    // 1. Colours, text and targets.
    let s = wait_for(&b, "carga", |s| {
        s.url.ends_with("index.html") && has_word(s, "Hola")
    });
    assert_eq!(s.title, "Prueba");
    let cw = s.viewport.0 / 100;
    assert_eq!(
        s.viewport.0,
        100 * cw,
        "el viewport es la rejilla en píxeles"
    );
    assert!(has_word(&s, "Cabecera") && has_word(&s, "roja"));
    assert!(has_word(&s, "visible-sobre-nada"));
    assert!(
        !has_word(&s, "tapado"),
        "el texto tapado por otro elemento no se dibuja"
    );
    assert!(
        !has_word(&s, "oculto"),
        "el texto recortado (solo para lectores) no se dibuja"
    );
    let link = s
        .targets
        .iter()
        .find(|t| t.kind == Kind::Link)
        .expect("un enlace");
    assert!(link.href.as_deref().unwrap().ends_with("segunda.html"));
    assert!(
        s.targets.iter().any(|t| t.kind == Kind::Input),
        "el campo de texto es un objetivo"
    );
    let grid = render(&s, 100, 30, None);
    let red = grid.get(70, 1).unwrap().bg;
    let blue = grid.get(70, 9).unwrap().bg;
    assert!(
        red.0 > 200 && red.1 < 40 && red.2 < 40,
        "banda roja, vista: {red:?}"
    );
    assert!(blue.2 > 200 && blue.0 < 40, "banda azul, vista: {blue:?}");
    let head: String = (0..30)
        .map(|c| grid.get(c, 1).map_or(' ', |c| c.ch))
        .collect();
    assert!(
        head.contains("Cabecera roja"),
        "el texto va sobre el rojo: {head:?}"
    );
    let letter = grid.get(1, 1).unwrap();
    assert!(
        letter.fg.0 > 200 && letter.fg.1 > 200,
        "texto blanco sobre rojo: {letter:?}"
    );

    // 2. Scrolling.
    b.send(Cmd::Scroll(300));
    let s = wait_for(&b, "scroll", |s| (s.scroll_y - 300.0).abs() < 2.0);
    assert!(s.page_height > 3000.0);
    b.send(Cmd::ScrollTop);
    wait_for(&b, "arriba", |s| s.scroll_y < 1.0);

    // 3. Click a link: navigates.
    let (x, y) = link.center();
    b.send(Cmd::Click(x, y));
    wait_for(&b, "enlace", |s| {
        s.url.ends_with("segunda.html") && has_word(s, "Segunda")
    });
    b.send(Cmd::Back);
    let s = wait_for(&b, "atrás", |s| {
        s.url.ends_with("index.html") && has_word(s, "Hola")
    });

    // 4. Type into a field.
    let (x, y) = s
        .targets
        .iter()
        .find(|t| t.kind == Kind::Input)
        .unwrap()
        .center();
    b.send(Cmd::Click(x, y));
    b.send(Cmd::Type("abc".into()));
    wait_for(&b, "escribir", |s| has_word(s, "abc"));

    // 5. A page that changes by itself: no command is sent after loading it.
    b.send(Cmd::Goto(url("dinamica.html")));
    wait_for(&b, "dinámica (antes)", |s| {
        s.url.ends_with("dinamica.html") && has_word(s, "antes")
    });
    let s = wait_for(&b, "dinámica (después)", |s| {
        has_word(s, "despues") && counter(s).is_some()
    });
    let n0 = counter(&s).unwrap();
    wait_for(&b, "dinámica (contador)", |s| {
        counter(s).is_some_and(|n| n >= n0 + 2)
    });

    // 6. A link that opens a new tab: the new tab is the one we show.
    b.send(Cmd::Goto(url("popup.html")));
    let s = wait_for(&b, "origen", |s| {
        s.url.ends_with("popup.html") && has_word(s, "pestaña")
    });
    let (x, y) = s
        .targets
        .iter()
        .find(|t| t.kind == Kind::Link)
        .unwrap()
        .center();
    b.send(Cmd::Click(x, y));
    wait_for(&b, "nueva pestaña", |s| {
        s.url.ends_with("segunda.html") && has_word(s, "Segunda")
    });

    // 7. Resizing the grid resizes the viewport.
    b.send(Cmd::Resize(60, 20));
    // Firefox does not go below ~500 px of window width, so the viewport may not be an exact
    // multiple of the columns; the renderer scales by viewport / columns.
    let s = wait_for(&b, "resize", |s| s.viewport.0 < 700);
    let grid = render(&s, 60, 20, None);
    assert_eq!((grid.cols, grid.rows), (60, 20));
    assert!(
        grid.cells.iter().any(|c| c.ch != ' '),
        "sigue habiendo texto tras redimensionar"
    );
    b.shutdown();
}
