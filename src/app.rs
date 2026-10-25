//! The terminal side: keys and mouse become browser commands, snapshots become cells.
//! Nothing here touches the terminal except `draw`, so the behaviour is testable.

use ratatui::Frame;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::browser::{Cmd, Event, Key};
use crate::page::{Kind, Rgb, Snapshot};
use crate::render::{Grid, render};
use crate::url::normalize;

const PAPER: Color = Color::Rgb(0x00, 0x00, 0x00);
const INK: Color = Color::Rgb(0xcd, 0xc4, 0xba);
const MUTED: Color = Color::Rgb(0x95, 0x8f, 0x87);
const RAISED: Color = Color::Rgb(0x1b, 0x1b, 0x1b);
const VERMILION: Color = Color::Rgb(0xe2, 0x34, 0x2a);
const GOLD: Color = Color::Rgb(0xd9, 0xa4, 0x41);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    Normal,
    /// Typing an address.
    Url(String),
    /// Keys go to the page (a text field has the focus).
    Input,
    Help,
}

pub struct App {
    /// Size of the page area, in cells.
    pub cols: u16,
    pub rows: u16,
    pub snap: Option<Box<Snapshot>>,
    grid: Option<Grid>,
    pub focus: Option<usize>,
    pub mode: Mode,
    pub status: String,
    pub failed: bool,
    pub quit: bool,
    pub dirty: bool,
}

const HELP: [(&str, &str); 15] = [
    ("j k  ↑ ↓", "desplazar"),
    ("Espacio / b", "una pantalla abajo / arriba"),
    ("g / G", "principio / final de la página"),
    ("Tab / Shift+Tab", "siguiente / anterior enlace o botón"),
    ("Enter", "abrir lo seleccionado"),
    ("Ctrl+L  o", "escribir una dirección o una búsqueda"),
    ("H / L", "atrás / adelante"),
    ("r", "recargar"),
    ("+ / -", "texto más grande / más pequeño"),
    ("Clic", "abrir lo que hay bajo el ratón"),
    ("Rueda", "desplazar"),
    ("Esc", "salir del modo escritura o quitar la selección"),
    ("?", "esta ayuda"),
    ("q  Ctrl+Q", "salir"),
    ("", "En un campo de texto: escribe; Esc para volver"),
];

impl App {
    pub fn new(cols: u16, rows: u16) -> App {
        App {
            cols: cols.max(1),
            rows: rows.max(1),
            snap: None,
            grid: None,
            focus: None,
            mode: Mode::Normal,
            status: "Iniciando Firefox…".into(),
            failed: false,
            quit: false,
            dirty: true,
        }
    }

    /// Pixels of page per row, or a guess before the first snapshot.
    fn row_px(&self) -> i32 {
        self.snap
            .as_ref()
            .map_or(16, |s| {
                (s.viewport.1 as f32 / self.rows as f32).round() as i32
            })
            .max(1)
    }

    fn col_px(&self) -> u32 {
        self.snap
            .as_ref()
            .map_or(8, |s| {
                (s.viewport.0 as f32 / self.cols as f32).round() as u32
            })
            .max(1)
    }

    fn invalidate(&mut self) {
        self.grid = None;
        self.dirty = true;
    }

    // ---- browser events -------------------------------------------------------------

    pub fn on_event(&mut self, ev: Event) {
        match ev {
            Event::Snapshot(s) => {
                let same_page = self.snap.as_ref().is_some_and(|o| o.url == s.url);
                if !same_page || self.focus.is_some_and(|i| i >= s.targets.len()) {
                    self.focus = None;
                }
                self.status.clear();
                self.failed = false;
                self.snap = Some(s);
            }
            Event::Status(m) => self.status = m,
            Event::Error(m) => {
                self.status = m;
                self.failed = true;
            }
            Event::Fatal(m) => {
                self.status = format!("{m}  (cualquier tecla sale)");
                self.failed = true;
            }
        }
        self.invalidate();
    }

    pub fn resize(&mut self, term_cols: u16, term_rows: u16) -> Option<Cmd> {
        let (c, r) = (term_cols.max(1), term_rows.saturating_sub(2).max(1));
        self.dirty = true;
        if (c, r) == (self.cols, self.rows) {
            return None;
        }
        self.cols = c;
        self.rows = r;
        self.grid = None;
        Some(Cmd::Resize(c, r))
    }

    // ---- keys -----------------------------------------------------------------------

    pub fn on_key(&mut self, key: KeyEvent) -> Vec<Cmd> {
        self.dirty = true;
        if self.failed && self.snap.is_none() {
            self.quit = true; // Firefox never came up: nothing to do but leave
            return vec![];
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match self.mode.clone() {
            Mode::Help => {
                self.mode = Mode::Normal;
                vec![]
            }
            Mode::Url(input) => self.key_url(key, input),
            Mode::Input => self.key_input(key),
            Mode::Normal => {
                let alt = key.modifiers.contains(KeyModifiers::ALT);
                let page = (self.rows as i32 - 2).max(1) * self.row_px();
                let line = 3 * self.row_px();
                match key.code {
                    KeyCode::Char('c' | 'q') if ctrl => self.quit = true,
                    KeyCode::Char('q') => self.quit = true,
                    KeyCode::Char('j') | KeyCode::Down => return vec![Cmd::Scroll(line)],
                    KeyCode::Char('k') | KeyCode::Up => return vec![Cmd::Scroll(-line)],
                    KeyCode::Char(' ') | KeyCode::PageDown => return vec![Cmd::Scroll(page)],
                    KeyCode::Char('b') | KeyCode::PageUp => return vec![Cmd::Scroll(-page)],
                    KeyCode::Char('g') | KeyCode::Home => return vec![Cmd::ScrollTop],
                    KeyCode::Char('G') | KeyCode::End => return vec![Cmd::ScrollBottom],
                    KeyCode::Char('l') if ctrl => self.mode = Mode::Url(String::new()),
                    KeyCode::Char('o') => self.mode = Mode::Url(String::new()),
                    KeyCode::Char('H') => return vec![Cmd::Back],
                    KeyCode::Left if alt => return vec![Cmd::Back],
                    KeyCode::Char('L') => return vec![Cmd::Forward],
                    KeyCode::Right if alt => return vec![Cmd::Forward],
                    KeyCode::Char('r') | KeyCode::F(5) => return vec![Cmd::Reload],
                    KeyCode::Char('+') | KeyCode::Char('=') => {
                        return vec![Cmd::CellWidth(self.col_px() + 1)];
                    }
                    KeyCode::Char('-') => {
                        return vec![Cmd::CellWidth(self.col_px().saturating_sub(1))];
                    }
                    KeyCode::Char('?') | KeyCode::F(1) => self.mode = Mode::Help,
                    KeyCode::Tab => self.move_focus(1),
                    KeyCode::BackTab => self.move_focus(-1),
                    KeyCode::Esc => {
                        self.focus = None;
                        self.invalidate();
                    }
                    KeyCode::Enter => return self.activate(),
                    _ => {}
                }
                vec![]
            }
        }
    }

    fn key_url(&mut self, key: KeyEvent, mut input: String) -> Vec<Cmd> {
        match key.code {
            KeyCode::Esc => self.mode = Mode::Normal,
            KeyCode::Enter => {
                self.mode = Mode::Normal;
                if !input.trim().is_empty() {
                    return vec![Cmd::Goto(normalize(&input))];
                }
            }
            KeyCode::Backspace => {
                input.pop();
                self.mode = Mode::Url(input);
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.mode = Mode::Url(String::new())
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                input.push(c);
                self.mode = Mode::Url(input);
            }
            _ => self.mode = Mode::Url(input),
        }
        vec![]
    }

    fn key_input(&mut self, key: KeyEvent) -> Vec<Cmd> {
        let special = match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                return vec![];
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                return vec![Cmd::Type(c.to_string())];
            }
            KeyCode::Enter => Key::Enter,
            KeyCode::Backspace => Key::Backspace,
            KeyCode::Delete => Key::Delete,
            KeyCode::Tab => Key::Tab,
            KeyCode::Left => Key::Left,
            KeyCode::Right => Key::Right,
            KeyCode::Up => Key::Up,
            KeyCode::Down => Key::Down,
            KeyCode::Home => Key::Home,
            KeyCode::End => Key::End,
            _ => return vec![],
        };
        vec![Cmd::Key(special)]
    }

    fn move_focus(&mut self, dir: i32) {
        let n = self.snap.as_ref().map_or(0, |s| s.targets.len());
        if n == 0 {
            self.status = "No hay enlaces ni botones en pantalla".into();
            return;
        }
        let next = match self.focus {
            None if dir > 0 => 0,
            None => n - 1,
            Some(i) => ((i as i32 + dir).rem_euclid(n as i32)) as usize,
        };
        self.focus = Some(next);
        self.status.clear();
        self.invalidate();
    }

    fn activate(&mut self) -> Vec<Cmd> {
        let Some(t) = self.focus.and_then(|i| self.snap.as_ref()?.targets.get(i)) else {
            return vec![];
        };
        let (x, y) = t.center();
        match t.kind {
            Kind::Input => {
                self.mode = Mode::Input;
                vec![Cmd::Click(x, y)]
            }
            Kind::Select => {
                self.status = "Las listas desplegables aún no están soportadas".into();
                vec![]
            }
            _ => vec![Cmd::Click(x, y)],
        }
    }

    // ---- mouse ----------------------------------------------------------------------

    /// Terminal cell → point of the viewport in CSS pixels.
    fn to_px(&self, col: u16, row: u16) -> Option<(f32, f32)> {
        let (vw, vh) = self.snap.as_ref()?.viewport;
        Some((
            (col as f32 + 0.5) * vw as f32 / self.cols as f32,
            (row as f32 + 0.5) * vh as f32 / self.rows as f32,
        ))
    }

    pub fn on_mouse(&mut self, m: MouseEvent) -> Vec<Cmd> {
        self.dirty = true;
        match m.kind {
            MouseEventKind::ScrollDown => vec![Cmd::Scroll(3 * self.row_px())],
            MouseEventKind::ScrollUp => vec![Cmd::Scroll(-3 * self.row_px())],
            MouseEventKind::Down(MouseButton::Left)
                if m.row >= 1 && m.row <= self.rows && m.column < self.cols =>
            {
                let Some((x, y)) = self.to_px(m.column, m.row - 1) else {
                    return vec![];
                };
                let on_input = self.snap.as_ref().is_some_and(|s| {
                    s.targets.iter().any(|t| {
                        t.kind == Kind::Input
                            && x >= t.x
                            && x <= t.x + t.w
                            && y >= t.y
                            && y <= t.y + t.h
                    })
                });
                self.mode = if on_input { Mode::Input } else { Mode::Normal };
                vec![Cmd::Click(x, y)]
            }
            _ => vec![],
        }
    }

    // ---- drawing --------------------------------------------------------------------

    pub fn draw(&mut self, f: &mut Frame) {
        let area = f.area();
        f.render_widget(Block::new().style(Style::new().bg(PAPER)), area);
        if area.height < 4 || area.width < 20 {
            f.render_widget(Paragraph::new("Terminal demasiado pequeño"), area);
            return;
        }
        self.draw_bar(f, Rect::new(0, 0, area.width, 1));
        self.draw_page(f, Rect::new(0, 1, area.width, area.height - 2));
        self.draw_footer(f, Rect::new(0, area.height - 1, area.width, 1));
        if self.mode == Mode::Help {
            draw_help(f, area);
        }
        if let Mode::Url(input) = &self.mode {
            let x = ("Ir a: ".chars().count() + input.chars().count()) as u16;
            f.set_cursor_position((x.min(area.width - 1), area.height - 1));
        }
    }

    fn draw_bar(&self, f: &mut Frame, r: Rect) {
        let title = self.snap.as_ref().map_or(String::new(), |s| {
            if s.title.is_empty() {
                s.url.clone()
            } else {
                format!("{}  ·  {}", s.title, s.url)
            }
        });
        let pct = self.snap.as_ref().map_or(String::new(), |s| {
            let max = (s.page_height - s.viewport.1 as f32).max(1.0);
            format!(
                " {:>3}% ",
                ((s.scroll_y / max) * 100.0).clamp(0.0, 100.0).round() as u32
            )
        });
        let w = r.width as usize;
        let brand = " webTUI ";
        let room = w.saturating_sub(brand.chars().count() + pct.chars().count() + 2);
        let shown: String = title.chars().take(room).collect();
        let pad = room.saturating_sub(shown.chars().count());
        let line = ratatui::text::Line::from(vec![
            ratatui::text::Span::styled(
                brand,
                Style::new()
                    .fg(PAPER)
                    .bg(VERMILION)
                    .add_modifier(Modifier::BOLD),
            ),
            ratatui::text::Span::styled(
                format!(" {shown}{} ", " ".repeat(pad)),
                Style::new().fg(INK).bg(RAISED),
            ),
            ratatui::text::Span::styled(pct, Style::new().fg(MUTED).bg(RAISED)),
        ]);
        f.render_widget(Paragraph::new(line).style(Style::new().bg(RAISED)), r);
    }

    fn draw_page(&mut self, f: &mut Frame, r: Rect) {
        let Some(snap) = &self.snap else {
            let msg = if self.status.is_empty() {
                "Escribe una dirección: Ctrl+L".to_string()
            } else {
                self.status.clone()
            };
            let y = r.y + r.height / 2;
            let x = r.x + r.width.saturating_sub(msg.chars().count() as u16) / 2;
            f.render_widget(
                Paragraph::new(msg).style(Style::new().fg(MUTED).bg(PAPER)),
                Rect::new(x, y, r.width.saturating_sub(x), 1),
            );
            return;
        };
        if self.grid.is_none() {
            self.grid = Some(render(snap, self.cols, self.rows, self.focus));
        }
        let grid = self.grid.as_ref().expect("grid just built");
        let buf = f.buffer_mut();
        for y in 0..grid.rows.min(r.height) {
            for x in 0..grid.cols.min(r.width) {
                let c = &grid.cells[y as usize * grid.cols as usize + x as usize];
                let cell = &mut buf[(r.x + x, r.y + y)];
                cell.set_char(c.ch).set_fg(rgb(c.fg)).set_bg(rgb(c.bg));
                if c.bold {
                    cell.modifier.insert(Modifier::BOLD);
                }
            }
        }
    }

    fn draw_footer(&self, f: &mut Frame, r: Rect) {
        let (text, style) = match &self.mode {
            Mode::Url(input) => (format!("Ir a: {input}"), Style::new().fg(GOLD).bg(PAPER)),
            Mode::Input => ("ESCRIBIENDO en la página · Esc para volver".to_string(), Style::new().fg(PAPER).bg(GOLD)),
            _ if self.failed => (self.status.clone(), Style::new().fg(VERMILION).bg(PAPER)),
            _ if !self.status.is_empty() => (self.status.clone(), Style::new().fg(GOLD).bg(PAPER)),
            _ => match self.focus.and_then(|i| self.snap.as_ref()?.targets.get(i)) {
                Some(t) => {
                    let what = t.href.clone().unwrap_or_else(|| t.label.clone());
                    (format!("{}  →  {}", t.label, what), Style::new().fg(GOLD).bg(PAPER))
                }
                None => (
                    "j/k desplazar · Tab enlaces · Enter abrir · Ctrl+L dirección · H atrás · ? ayuda · q salir".to_string(),
                    Style::new().fg(MUTED).bg(PAPER),
                ),
            },
        };
        f.render_widget(Paragraph::new(text).style(style), r);
    }
}

fn rgb(c: Rgb) -> Color {
    Color::Rgb(c.0, c.1, c.2)
}

fn draw_help(f: &mut Frame, area: Rect) {
    let h = HELP.len() as u16 + 4;
    let w = 72.min(area.width);
    let r = Rect::new(
        area.x + (area.width - w) / 2,
        area.y + area.height.saturating_sub(h) / 2,
        w,
        h.min(area.height),
    );
    f.render_widget(Clear, r);
    let lines: Vec<ratatui::text::Line> = HELP
        .iter()
        .map(|(k, d)| {
            ratatui::text::Line::from(vec![
                ratatui::text::Span::styled(format!(" {k:<18}"), Style::new().fg(GOLD).bg(PAPER)),
                ratatui::text::Span::styled(*d, Style::new().fg(INK).bg(PAPER)),
            ])
        })
        .collect();
    let block = Block::new()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(MUTED))
        .title(" AYUDA // webTUI ")
        .style(Style::new().bg(PAPER));
    f.render_widget(Paragraph::new(lines).block(block), r);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page::{Target, test_png};
    use image::RgbImage;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyEventKind, KeyEventState, MouseEvent};
    use serde_json::json;

    fn press(app: &mut App, code: KeyCode) -> Vec<Cmd> {
        app.on_key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn snap(targets: Vec<Target>) -> Box<Snapshot> {
        let img = RgbImage::from_pixel(160, 64, image::Rgb([255, 255, 255]));
        let v = json!({"url": "https://a.org/", "title": "Aaa", "vw": 160, "vh": 64, "sy": 0.0, "ph": 640.0});
        let mut s = Snapshot::from_json(&v, &test_png(&img)).unwrap();
        s.targets = targets;
        Box::new(s)
    }

    fn target(kind: Kind, x: f32, y: f32) -> Target {
        Target {
            kind,
            x,
            y,
            w: 16.0,
            h: 16.0,
            label: "t".into(),
            href: Some("https://a.org/x".into()),
        }
    }

    /// 20 columns × 4 rows → 8 × 16 px cells.
    fn app() -> App {
        let mut a = App::new(20, 4);
        a.on_event(Event::Snapshot(snap(vec![
            target(Kind::Link, 8.0, 0.0),
            target(Kind::Input, 32.0, 16.0),
        ])));
        a
    }

    #[test]
    fn scrolling_keys_move_by_whole_rows() {
        let mut a = app();
        assert_eq!(press(&mut a, KeyCode::Char('j')), vec![Cmd::Scroll(48)]);
        assert_eq!(press(&mut a, KeyCode::Up), vec![Cmd::Scroll(-48)]);
        assert_eq!(press(&mut a, KeyCode::Char(' ')), vec![Cmd::Scroll(32)]);
        assert_eq!(press(&mut a, KeyCode::Char('g')), vec![Cmd::ScrollTop]);
        assert_eq!(press(&mut a, KeyCode::Char('G')), vec![Cmd::ScrollBottom]);
        assert_eq!(press(&mut a, KeyCode::Char('H')), vec![Cmd::Back]);
        assert_eq!(press(&mut a, KeyCode::Char('r')), vec![Cmd::Reload]);
        assert_eq!(press(&mut a, KeyCode::Char('+')), vec![Cmd::CellWidth(9)]);
    }

    #[test]
    fn the_address_bar_turns_text_into_a_url() {
        let mut a = app();
        a.on_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL));
        assert_eq!(a.mode, Mode::Url(String::new()));
        for c in "example.com".chars() {
            press(&mut a, KeyCode::Char(c));
        }
        press(&mut a, KeyCode::Backspace);
        press(&mut a, KeyCode::Char('m'));
        assert_eq!(
            press(&mut a, KeyCode::Enter),
            vec![Cmd::Goto("https://example.com".into())]
        );
        assert_eq!(a.mode, Mode::Normal);
        press(&mut a, KeyCode::Char('o'));
        press(&mut a, KeyCode::Esc);
        assert_eq!(a.mode, Mode::Normal);
        press(&mut a, KeyCode::Char('o'));
        assert!(
            press(&mut a, KeyCode::Enter).is_empty(),
            "an empty address goes nowhere"
        );
    }

    #[test]
    fn tab_walks_the_targets_and_enter_opens_them() {
        let mut a = app();
        assert!(
            press(&mut a, KeyCode::Enter).is_empty(),
            "nothing selected yet"
        );
        press(&mut a, KeyCode::Tab);
        assert_eq!(a.focus, Some(0));
        assert_eq!(press(&mut a, KeyCode::Enter), vec![Cmd::Click(16.0, 8.0)]);
        press(&mut a, KeyCode::Tab);
        assert_eq!(a.focus, Some(1));
        press(&mut a, KeyCode::Tab);
        assert_eq!(a.focus, Some(0), "wraps around");
        a.on_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
        assert_eq!(a.focus, Some(1));
        // The second target is a text field: Enter clicks it and typing goes to the page.
        assert_eq!(press(&mut a, KeyCode::Enter), vec![Cmd::Click(40.0, 24.0)]);
        assert_eq!(a.mode, Mode::Input);
        assert_eq!(
            press(&mut a, KeyCode::Char('q')),
            vec![Cmd::Type("q".into())]
        );
        assert!(!a.quit, "q is text while typing");
        assert_eq!(
            press(&mut a, KeyCode::Backspace),
            vec![Cmd::Key(Key::Backspace)]
        );
        assert_eq!(press(&mut a, KeyCode::Enter), vec![Cmd::Key(Key::Enter)]);
        assert!(press(&mut a, KeyCode::Esc).is_empty());
        assert_eq!(a.mode, Mode::Normal);
    }

    #[test]
    fn selection_is_dropped_when_the_page_changes() {
        let mut a = app();
        press(&mut a, KeyCode::Tab);
        a.on_event(Event::Snapshot(snap(vec![
            target(Kind::Link, 0.0, 0.0),
            target(Kind::Link, 0.0, 16.0),
        ])));
        assert_eq!(a.focus, Some(0), "same page: the selection stays");
        let mut s = snap(vec![]);
        s.url = "https://b.org/".into();
        a.on_event(Event::Snapshot(s));
        assert_eq!(a.focus, None);
    }

    #[test]
    fn mouse_clicks_map_cells_to_page_pixels() {
        let mut a = app();
        // Row 1 of the terminal is the first row of the page.
        let click = |col, row| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(a.on_mouse(click(2, 1)), vec![Cmd::Click(20.0, 8.0)]);
        assert!(
            a.on_mouse(click(2, 0)).is_empty(),
            "the title bar is not the page"
        );
        assert!(a.on_mouse(click(2, 5)).is_empty(), "nor the status line");
        // Clicking on the text field enters typing mode.
        a.on_mouse(click(5, 2));
        assert_eq!(a.mode, Mode::Input);
        a.on_mouse(click(15, 4));
        assert_eq!(a.mode, Mode::Normal);
        let wheel = MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 0,
            row: 1,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(a.on_mouse(wheel), vec![Cmd::Scroll(48)]);
    }

    #[test]
    fn resizing_the_terminal_resizes_the_page_area() {
        let mut a = app();
        assert_eq!(a.resize(100, 32), Some(Cmd::Resize(100, 30)));
        assert_eq!(a.resize(100, 32), None);
        assert_eq!((a.cols, a.rows), (100, 30));
    }

    #[test]
    fn a_dead_browser_lets_any_key_leave() {
        let mut a = App::new(20, 4);
        a.on_event(Event::Fatal("Firefox no arranca".into()));
        assert!(a.failed && !a.quit);
        press(&mut a, KeyCode::Char('x'));
        assert!(a.quit);
    }

    #[test]
    fn draws_bar_page_and_footer() {
        let mut a = app();
        let mut t = Terminal::new(TestBackend::new(20, 6)).unwrap();
        t.draw(|f| a.draw(f)).unwrap();
        let b = t.backend().buffer();
        let row = |y: u16| {
            (0..20)
                .map(|x| b[(x, y)].symbol().to_string())
                .collect::<String>()
        };
        assert!(
            row(0).contains("webTUI") && row(0).contains("Aaa"),
            "{:?}",
            row(0)
        );
        assert_eq!(
            b[(3, 2)].bg,
            Color::Rgb(255, 255, 255),
            "the page is painted with its colours"
        );
        assert!(row(5).contains("j/k"), "{:?}", row(5));
        press(&mut a, KeyCode::Char('?'));
        t.draw(|f| a.draw(f)).unwrap();
        let all: String = (0..6)
            .map(|y| {
                (0..20)
                    .map(|x| t.backend().buffer()[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect();
        assert!(all.contains("AYUDA"));
        let _ = (KeyEventKind::Press, KeyEventState::NONE);
    }

    #[test]
    fn a_tiny_terminal_does_not_panic() {
        let mut a = app();
        for (w, h) in [(1, 1), (19, 3), (20, 4), (40, 5)] {
            let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
            t.draw(|f| a.draw(f)).unwrap();
        }
    }
}
