//! Turns a [`Snapshot`] into a grid of terminal cells. Each cell is two pixel rows (`▀` with
//! the colour of the upper half as foreground and the lower half as background); the real
//! text of the page is drawn on top, character by character, with its own colour.

use unicode_width::UnicodeWidthChar;

use crate::page::{Rgb, Snapshot, Target};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub fg: Rgb,
    pub bg: Rgb,
    pub bold: bool,
}

pub struct Grid {
    pub cols: u16,
    pub rows: u16,
    pub cells: Vec<Cell>,
}

impl Grid {
    pub fn get(&self, x: u16, y: u16) -> Option<&Cell> {
        (x < self.cols && y < self.rows)
            .then(|| &self.cells[y as usize * self.cols as usize + x as usize])
    }
}

/// Two colours closer than this (sum of channel differences) are drawn as one flat colour.
const FLAT: i32 = 48;
/// Takumi inversion colours (ink on black / black on ink) for the selected target.
fn inverse_pair() -> (Rgb, Rgb) {
    use crate::takumi::oscuro::{INVERSO_FONDO, INVERSO_TINTA};
    use ratatui::style::Color;
    let rgb = |c: Color| match c {
        Color::Rgb(r, g, b) => Rgb(r, g, b),
        _ => Rgb(0, 0, 0),
    };
    (rgb(INVERSO_FONDO), rgb(INVERSO_TINTA))
}

fn dist(a: Rgb, b: Rgb) -> i32 {
    (a.0 as i32 - b.0 as i32).abs()
        + (a.1 as i32 - b.1 as i32).abs()
        + (a.2 as i32 - b.2 as i32).abs()
}

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let m = |x: u8, y: u8| (x as f32 * (1.0 - t) + y as f32 * t).round() as u8;
    Rgb(m(a.0, b.0), m(a.1, b.1), m(a.2, b.2))
}

fn lum(c: Rgb) -> f32 {
    0.2126 * c.0 as f32 + 0.7152 * c.1 as f32 + 0.0722 * c.2 as f32
}

/// Foreground that can be read over `bg`: keep `fg` unless the two are too close.
fn readable(fg: Rgb, bg: Rgb) -> Rgb {
    if (lum(fg) - lum(bg)).abs() >= 70.0 {
        fg
    } else if lum(bg) < 128.0 {
        Rgb(255, 255, 255)
    } else {
        Rgb(0, 0, 0)
    }
}

/// Mean colour of the pixel box `[x0,x1) × [y0,y1)` (at least one pixel).
fn average(img: &image::RgbImage, x0: f32, y0: f32, x1: f32, y1: f32) -> Rgb {
    let (w, h) = img.dimensions();
    let xa = (x0.floor().max(0.0) as u32).min(w.saturating_sub(1));
    let ya = (y0.floor().max(0.0) as u32).min(h.saturating_sub(1));
    let xb = (x1.ceil() as u32).clamp(xa + 1, w);
    let yb = (y1.ceil() as u32).clamp(ya + 1, h);
    let (mut r, mut g, mut b, mut n) = (0u64, 0u64, 0u64, 0u64);
    for y in ya..yb {
        for x in xa..xb {
            let p = img.get_pixel(x, y).0;
            r += p[0] as u64;
            g += p[1] as u64;
            b += p[2] as u64;
            n += 1;
        }
    }
    Rgb((r / n) as u8, (g / n) as u8, (b / n) as u8)
}

/// Render `snap` into `cols` × `rows` cells. `focus` is the index of the selected target.
pub fn render(snap: &Snapshot, cols: u16, rows: u16, focus: Option<usize>) -> Grid {
    let (vw, vh) = (snap.viewport.0.max(1) as f32, snap.viewport.1.max(1) as f32);
    // CSS pixels per cell, and screenshot pixels per CSS pixel (device pixel ratio).
    let (cw, ch) = (vw / cols as f32, vh / rows as f32);
    let (sx, sy) = (
        snap.image.width() as f32 / vw,
        snap.image.height() as f32 / vh,
    );

    let mut cells = Vec::with_capacity(cols as usize * rows as usize);
    for r in 0..rows {
        for c in 0..cols {
            let (x0, x1) = (c as f32 * cw * sx, (c + 1) as f32 * cw * sx);
            let y0 = r as f32 * ch * sy;
            let mid = y0 + ch * sy / 2.0;
            let top = average(&snap.image, x0, y0, x1, mid);
            let bot = average(&snap.image, x0, mid, x1, mid + ch * sy / 2.0);
            cells.push(if dist(top, bot) < FLAT {
                let m = mix(top, bot, 0.5);
                Cell {
                    ch: ' ',
                    fg: m,
                    bg: m,
                    bold: false,
                }
            } else {
                Cell {
                    ch: '\u{2580}',
                    fg: top,
                    bg: bot,
                    bold: false,
                }
            });
        }
    }
    // Background of every cell as it is without text: what the text is drawn over.
    let back: Vec<Rgb> = cells
        .iter()
        .map(|c| {
            if c.ch == ' ' {
                c.bg
            } else {
                mix(c.fg, c.bg, 0.5)
            }
        })
        .collect();

    if let Some(t) = focus.and_then(|i| snap.targets.get(i)) {
        tint(&mut cells, &back, cols, rows, t, cw, ch);
    }

    // Words left to right, row by row, so a long word pushes the next one instead of
    // overwriting it.
    let mut order: Vec<usize> = (0..snap.words.len()).collect();
    let row_of = |i: usize| ((snap.words[i].y + snap.words[i].h / 2.0) / ch).floor() as i32;
    order.sort_by(|&a, &b| {
        row_of(a)
            .cmp(&row_of(b))
            .then(snap.words[a].x.total_cmp(&snap.words[b].x))
    });
    // Per row: first free column and where (in pixels) the last word ended.
    let mut free = vec![0usize; rows as usize];
    let mut ended = vec![f32::NEG_INFINITY; rows as usize];
    for i in order {
        let w = &snap.words[i];
        let r = row_of(i);
        if r < 0 || r >= rows as i32 {
            continue;
        }
        let r = r as usize;
        // Words that touch in the page (`kernel` + `.`) stay together; otherwise one blank.
        let gap = usize::from((w.x - ended[r]).abs() > 1.0 && free[r] > 0);
        let mut col = ((w.x / cw).round().max(0.0) as usize).max(free[r] + gap);
        for chr in w.text.chars() {
            let width = chr.width().unwrap_or(0);
            if width == 0 {
                continue;
            }
            if col + width > cols as usize {
                break;
            }
            let idx = r * cols as usize + col;
            let bg = cells[idx].bg_for_text(back[idx]);
            cells[idx] = Cell {
                ch: chr,
                fg: readable(w.fg, bg),
                bg,
                bold: w.bold,
            };
            if width == 2 {
                // The second half of a wide character: blank, same colours.
                let j = idx + 1;
                cells[j] = Cell {
                    ch: ' ',
                    fg: cells[idx].fg,
                    bg: cells[idx].bg,
                    bold: false,
                };
            }
            col += width;
        }
        free[r] = col;
        ended[r] = w.x + w.w;
    }
    // The selected target is a pure inversion: its text takes the opposite colour of the block.
    if let Some(t) = focus.and_then(|i| snap.targets.get(i)) {
        let (ink, paper) = inverse_pair();
        let c0 = (t.x / cw).floor().max(0.0) as u16;
        let c1 = (((t.x + t.w) / cw).ceil() as u16).min(cols);
        let r0 = (t.y / ch).floor().max(0.0) as u16;
        let r1 = (((t.y + t.h) / ch).ceil() as u16).min(rows);
        for r in r0..r1 {
            for c in c0..c1 {
                let cell = &mut cells[r as usize * cols as usize + c as usize];
                if cell.ch != ' ' {
                    cell.fg = if cell.bg == ink { paper } else { ink };
                }
            }
        }
    }
    Grid { cols, rows, cells }
}

impl Cell {
    /// Background under a character: a tinted cell keeps its tint, otherwise the cell's own.
    fn bg_for_text(&self, back: Rgb) -> Rgb {
        if self.ch == ' ' { self.bg } else { back }
    }
}

/// Paint the selected target by inversion: a light page area becomes black, a dark one ink.
fn tint(cells: &mut [Cell], back: &[Rgb], cols: u16, rows: u16, t: &Target, cw: f32, ch: f32) {
    let c0 = (t.x / cw).floor().max(0.0) as u16;
    let c1 = (((t.x + t.w) / cw).ceil() as u16).min(cols);
    let r0 = (t.y / ch).floor().max(0.0) as u16;
    let r1 = (((t.y + t.h) / ch).ceil() as u16).min(rows);
    for r in r0..r1 {
        for c in c0..c1 {
            let i = r as usize * cols as usize + c as usize;
            let (ink, paper) = inverse_pair();
            let bg = if lum(back[i]) >= 128.0 { paper } else { ink };
            cells[i] = Cell {
                ch: ' ',
                fg: bg,
                bg,
                bold: false,
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page::{Kind, Word, test_png};
    use image::RgbImage;
    use serde_json::json;

    /// A snapshot over `img`, whose viewport is `cols*8 × rows*16` CSS pixels.
    fn snap(img: RgbImage, words: Vec<Word>) -> Snapshot {
        let v = json!({"vw": img.width(), "vh": img.height()});
        let mut s = Snapshot::from_json(&v, &test_png(&img)).unwrap();
        s.words = words;
        s
    }

    fn word(t: &str, x: f32, y: f32, fg: Rgb) -> Word {
        Word {
            text: t.into(),
            x,
            y,
            w: t.len() as f32 * 8.0,
            h: 16.0,
            fg,
            bold: false,
        }
    }

    fn text_row(g: &Grid, r: u16) -> String {
        (0..g.cols).map(|c| g.get(c, r).unwrap().ch).collect()
    }

    #[test]
    fn flat_areas_become_one_colour_and_halves_use_the_half_block() {
        let mut img = RgbImage::from_pixel(32, 32, image::Rgb([10, 20, 30]));
        for y in 16..32 {
            for x in 0..32 {
                img.put_pixel(x, y, image::Rgb([200, 0, 0]));
            }
        }
        // 4 columns × 2 rows: the first row is flat, the second is flat red; no split cells.
        let g = render(&snap(img.clone(), vec![]), 4, 2, None);
        assert_eq!(g.get(0, 0).unwrap().bg, Rgb(10, 20, 30));
        assert_eq!(g.get(3, 1).unwrap().bg, Rgb(200, 0, 0));
        // 4 columns × 1 row: each cell covers both colours.
        let g = render(&snap(img, vec![]), 4, 1, None);
        let c = g.get(1, 0).unwrap();
        assert_eq!(
            (c.ch, c.fg, c.bg),
            ('\u{2580}', Rgb(10, 20, 30), Rgb(200, 0, 0))
        );
    }

    #[test]
    fn text_goes_where_the_word_was_with_its_colour() {
        let img = RgbImage::from_pixel(80, 32, image::Rgb([255, 255, 255]));
        let g = render(
            &snap(img, vec![word("hola", 16.0, 16.0, Rgb(0, 0, 200))]),
            10,
            2,
            None,
        );
        assert_eq!(text_row(&g, 1), "  hola    ");
        assert_eq!(g.get(2, 1).unwrap().fg, Rgb(0, 0, 200));
        assert_eq!(g.get(2, 1).unwrap().bg, Rgb(255, 255, 255));
        assert_eq!(text_row(&g, 0).trim(), "");
    }

    #[test]
    fn words_keep_reading_order_and_never_overwrite_each_other() {
        let img = RgbImage::from_pixel(160, 16, image::Rgb([255, 255, 255]));
        // Given out of order and too close: "uno" ends at column 5, "dos" starts at column 3.
        let ws = vec![
            word("dos", 24.0, 0.0, Rgb(0, 0, 0)),
            word("uno", 16.0, 0.0, Rgb(0, 0, 0)),
        ];
        let g = render(&snap(img, ws), 20, 1, None);
        assert_eq!(text_row(&g, 0).trim_end(), "  uno dos");
    }

    #[test]
    fn words_that_touch_stay_together_and_others_get_a_blank() {
        let img = RgbImage::from_pixel(160, 16, image::Rgb([255, 255, 255]));
        let black = Rgb(0, 0, 0);
        // "kernel" ends exactly where "." starts; then a real gap before "Otra".
        let ws = vec![
            word("kernel", 0.0, 0.0, black),
            word(".", 48.0, 0.0, black),
            word("Otra", 60.0, 0.0, black),
        ];
        let g = render(&snap(img, ws), 20, 1, None);
        assert_eq!(text_row(&g, 0).trim_end(), "kernel. Otra");
    }

    #[test]
    fn long_words_are_cut_at_the_right_edge() {
        let img = RgbImage::from_pixel(48, 16, image::Rgb([255, 255, 255]));
        let g = render(
            &snap(img, vec![word("abcdefghij", 24.0, 0.0, Rgb(0, 0, 0))]),
            6,
            1,
            None,
        );
        assert_eq!(text_row(&g, 0), "   abc");
    }

    #[test]
    fn wide_characters_take_two_cells() {
        let img = RgbImage::from_pixel(80, 16, image::Rgb([255, 255, 255]));
        let g = render(
            &snap(img, vec![word("日a", 0.0, 0.0, Rgb(0, 0, 0))]),
            10,
            1,
            None,
        );
        assert_eq!(
            (
                g.get(0, 0).unwrap().ch,
                g.get(1, 0).unwrap().ch,
                g.get(2, 0).unwrap().ch
            ),
            ('日', ' ', 'a')
        );
    }

    #[test]
    fn text_too_close_to_its_background_is_made_readable() {
        let dark = RgbImage::from_pixel(80, 16, image::Rgb([10, 10, 10]));
        let g = render(
            &snap(dark, vec![word("x", 0.0, 0.0, Rgb(20, 20, 20))]),
            10,
            1,
            None,
        );
        assert_eq!(g.get(0, 0).unwrap().fg, Rgb(255, 255, 255));
        let light = RgbImage::from_pixel(80, 16, image::Rgb([240, 240, 240]));
        let g = render(
            &snap(light, vec![word("x", 0.0, 0.0, Rgb(250, 250, 250))]),
            10,
            1,
            None,
        );
        assert_eq!(g.get(0, 0).unwrap().fg, Rgb(0, 0, 0));
    }

    #[test]
    fn the_focused_target_is_tinted_but_its_text_stays() {
        let img = RgbImage::from_pixel(80, 32, image::Rgb([255, 255, 255]));
        let mut s = snap(img, vec![word("ir", 8.0, 0.0, Rgb(120, 120, 120))]);
        s.targets.push(Target {
            kind: Kind::Link,
            x: 8.0,
            y: 0.0,
            w: 16.0,
            h: 16.0,
            label: "ir".into(),
            href: None,
        });
        let g = render(&s, 10, 2, Some(0));
        assert_eq!(text_row(&g, 0).trim(), "ir");
        assert_eq!(
            g.get(1, 0).unwrap().bg,
            Rgb(0, 0, 0),
            "a light area is inverted to black"
        );
        assert_eq!(
            g.get(1, 0).unwrap().fg,
            Rgb(242, 242, 242),
            "its text is forced to the opposite colour, even if the page colour was legible"
        );
        assert_eq!(
            g.get(5, 0).unwrap().bg,
            Rgb(255, 255, 255),
            "outside the target nothing changes"
        );
        let g = render(&s, 10, 2, None);
        assert_eq!(g.get(1, 0).unwrap().bg, Rgb(255, 255, 255));
    }

    #[test]
    fn a_focused_target_on_a_dark_page_is_inverted_to_ink() {
        let img = RgbImage::from_pixel(80, 32, image::Rgb([5, 5, 5]));
        let mut s = snap(img, vec![word("ir", 8.0, 0.0, Rgb(240, 240, 240))]);
        s.targets.push(Target {
            kind: Kind::Link,
            x: 8.0,
            y: 0.0,
            w: 16.0,
            h: 16.0,
            label: "ir".into(),
            href: None,
        });
        let g = render(&s, 10, 2, Some(0));
        assert_eq!(g.get(1, 0).unwrap().bg, Rgb(242, 242, 242));
        assert_eq!(g.get(1, 0).unwrap().fg, Rgb(0, 0, 0));
    }

    #[test]
    fn a_scaled_screenshot_still_maps_to_the_same_cells() {
        // Device pixel ratio 2: the image is twice the viewport.
        let img = RgbImage::from_pixel(160, 64, image::Rgb([5, 6, 7]));
        let mut s = snap(img, vec![]);
        s.viewport = (80, 32);
        let g = render(&s, 10, 2, None);
        assert_eq!(g.get(9, 1).unwrap().bg, Rgb(5, 6, 7));
    }
}
