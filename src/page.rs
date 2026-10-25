//! What we know about a page at one instant: its visible words, the things you can click,
//! and a screenshot taken with the text hidden (so only backgrounds and images remain).

use image::RgbImage;
use serde_json::Value;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

#[derive(Clone, Debug, PartialEq)]
pub struct Word {
    pub text: String,
    /// Position and size in CSS pixels, relative to the viewport.
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub fg: Rgb,
    pub bold: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Link,
    Button,
    /// Text field: typing goes to the page.
    Input,
    Select,
    Other,
}

/// Something that reacts to a click, already clipped to the viewport.
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub kind: Kind,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub label: String,
    pub href: Option<String>,
}

impl Target {
    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
}

pub struct Snapshot {
    pub url: String,
    pub title: String,
    /// Viewport size in CSS pixels.
    pub viewport: (u32, u32),
    pub scroll_y: f32,
    pub page_height: f32,
    pub words: Vec<Word>,
    pub targets: Vec<Target>,
    pub image: RgbImage,
}

/// Collects everything in one round trip. Returns an object; see [`Snapshot::from_json`].
pub const COLLECT_JS: &str = r#"
const parse = c => { const m = (c.match(/[\d.]+/g) || [0, 0, 0]).map(Number); return [m[0] | 0, m[1] | 0, m[2] | 0, m[3] === undefined ? 1 : m[3]]; };
const vw = innerWidth, vh = innerHeight;
const shown = el => typeof el.checkVisibility !== 'function' || el.checkVisibility({ checkOpacity: true, checkVisibilityCSS: true });
const inView = b => b.width > 0 && b.height > 0 && b.bottom > 0 && b.top < vh && b.right > 0 && b.left < vw;
const words = [];
const skip = new Set(['SCRIPT', 'STYLE', 'NOSCRIPT', 'TEXTAREA', 'SELECT', 'OPTION', 'TITLE', 'HEAD']);
const root = document.body || document.documentElement;
const tw = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
let n;
while ((n = tw.nextNode()) && words.length < 8000) {
  const p = n.parentElement;
  if (!p || skip.has(p.tagName) || !n.nodeValue.trim()) continue;
  const b0 = p.getBoundingClientRect();
  if (!inView(b0) && p.offsetParent !== null && b0.height > 0) continue;
  if (!shown(p)) continue;
  const cs = getComputedStyle(p);
  // A tiny clipped box whose text overflows it: "visually hidden" (skip links, screen-reader labels).
  if ((b0.width <= 2 || b0.height <= 2) && (cs.overflow !== 'visible' || (cs.clip && cs.clip !== 'auto') || (cs.clipPath && cs.clipPath !== 'none'))) continue;
  const col = parse(cs.color);
  const re = /\S+/g; let m; const t = n.nodeValue;
  while ((m = re.exec(t)) && words.length < 8000) {
    const r = document.createRange(); r.setStart(n, m.index); r.setEnd(n, m.index + m[0].length);
    const b = r.getBoundingClientRect();
    if (!inView(b)) continue;
    // Hidden behind something else (stacked carousel slides, modals) or clipped away
    // (visually-hidden "skip to content" links): the element at that point is not related.
    if (cs.pointerEvents !== 'none') {
      const top = document.elementFromPoint(Math.min(vw - 1, Math.max(0, b.left + b.width / 2)), Math.min(vh - 1, Math.max(0, b.top + b.height / 2)));
      if (top && !(p.contains(top) || top.contains(p))) continue;
    }
    words.push({ t: m[0], x: b.left, y: b.top, w: b.width, h: b.height, c: col, b: +cs.fontWeight >= 600 });
  }
}
for (const el of document.querySelectorAll('input:not([type=hidden]):not([type=checkbox]):not([type=radio]), textarea')) {
  const b = el.getBoundingClientRect(); if (!inView(b) || !shown(el)) continue;
  const v = el.value || el.placeholder || ''; if (!v) continue;
  const cs = getComputedStyle(el); const c = parse(cs.color);
  const dim = el.value ? 1 : 0.6;
  const fs = parseFloat(cs.fontSize) || 14;
  const padL = parseFloat(cs.paddingLeft) || 0;
  let x = b.left + padL + 2;
  for (const w of v.split(/\s+/).filter(Boolean).slice(0, 40)) {
    words.push({ t: w, x, y: b.top + (b.height - fs) / 2, w: w.length * fs * 0.55, h: fs, c: c.map((q, i) => i < 3 ? Math.round(q * dim + 128 * (1 - dim)) : q), b: false });
    x += (w.length + 1) * fs * 0.55;
  }
}
for (const el of document.querySelectorAll('select')) {
  const b = el.getBoundingClientRect(); if (!inView(b) || !shown(el)) continue;
  const o = el.options[el.selectedIndex]; if (!o) continue;
  const cs = getComputedStyle(el);
  words.push({ t: o.text.trim().split(/\s+/)[0] || '', x: b.left + 4, y: b.top, w: b.width, h: b.height, c: parse(cs.color), b: false });
}
const sel = 'a[href], button, input:not([type=hidden]), textarea, select, summary, [role=button], [role=link], [role=tab], [onclick]';
const targets = [];
for (const el of document.querySelectorAll(sel)) {
  if (targets.length >= 400) break;
  const b = el.getBoundingClientRect(); if (!inView(b) || !shown(el)) continue;
  const x = Math.max(0, b.left), y = Math.max(0, b.top), w = Math.min(vw, b.right) - x, h = Math.min(vh, b.bottom) - y;
  if (w < 2 || h < 2) continue;
  const top = document.elementFromPoint(x + w / 2, y + h / 2);
  if (!top || !(top === el || el.contains(top) || top.contains(el))) continue;
  const tag = el.tagName; const ty = (el.getAttribute('type') || '').toLowerCase();
  let k = 'other';
  if (tag === 'A' || el.getAttribute('role') === 'link') k = 'link';
  else if (tag === 'BUTTON' || el.getAttribute('role') === 'button' || ['submit', 'button', 'reset', 'checkbox', 'radio', 'image'].includes(ty)) k = 'button';
  else if (tag === 'INPUT' || tag === 'TEXTAREA' || el.isContentEditable) k = 'input';
  else if (tag === 'SELECT') k = 'select';
  const label = (el.getAttribute('aria-label') || el.innerText || el.value || el.placeholder || el.title || el.alt || '').trim().replace(/\s+/g, ' ').slice(0, 80);
  targets.push({ k, x, y, w, h, l: label, h2: tag === 'A' ? el.href : null });
}
return { url: location.href, title: document.title, vw, vh, sy: scrollY, ph: Math.max(document.documentElement.scrollHeight, document.body ? document.body.scrollHeight : 0), words, targets };
"#;

/// Hides every text so the screenshot only holds backgrounds, borders and images.
pub const HIDE_TEXT_JS: &str = r#"
let s = document.getElementById('webtui-hide');
if (!s) {
  s = document.createElement('style'); s.id = 'webtui-hide';
  s.textContent = '*, *::before, *::after { color: transparent !important; text-shadow: none !important; -webkit-text-fill-color: transparent !important; caret-color: transparent !important; text-decoration-color: transparent !important; } ::placeholder { color: transparent !important; }';
  (document.head || document.documentElement).appendChild(s);
}
"#;

pub const SHOW_TEXT_JS: &str =
    "const s = document.getElementById('webtui-hide'); if (s) s.remove();";

fn num(v: &Value) -> f32 {
    v.as_f64().unwrap_or(0.0) as f32
}

fn rgb(v: &Value) -> Rgb {
    let c = |i: usize| v[i].as_u64().unwrap_or(0).min(255) as u8;
    Rgb(c(0), c(1), c(2))
}

impl Snapshot {
    /// Build a snapshot from the answer of [`COLLECT_JS`] and the PNG of the page.
    pub fn from_json(v: &Value, png: &[u8]) -> Result<Snapshot, String> {
        let image = image::load_from_memory_with_format(png, image::ImageFormat::Png)
            .map_err(|e| format!("captura no válida: {e}"))?
            .to_rgb8();
        let words = v["words"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|w| {
                        let text = w["t"].as_str()?.to_string();
                        (!text.is_empty()).then(|| Word {
                            text,
                            x: num(&w["x"]),
                            y: num(&w["y"]),
                            w: num(&w["w"]),
                            h: num(&w["h"]),
                            fg: rgb(&w["c"]),
                            bold: w["b"].as_bool().unwrap_or(false),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let targets = v["targets"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|t| Target {
                        kind: match t["k"].as_str() {
                            Some("link") => Kind::Link,
                            Some("button") => Kind::Button,
                            Some("input") => Kind::Input,
                            Some("select") => Kind::Select,
                            _ => Kind::Other,
                        },
                        x: num(&t["x"]),
                        y: num(&t["y"]),
                        w: num(&t["w"]),
                        h: num(&t["h"]),
                        label: t["l"].as_str().unwrap_or("").to_string(),
                        href: t["h2"].as_str().map(String::from),
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(Snapshot {
            url: v["url"].as_str().unwrap_or("").to_string(),
            title: v["title"].as_str().unwrap_or("").to_string(),
            viewport: (num(&v["vw"]) as u32, num(&v["vh"]) as u32),
            scroll_y: num(&v["sy"]),
            page_height: num(&v["ph"]),
            words,
            targets,
            image,
        })
    }

    /// Median width of one character, from words with at least 3 letters.
    pub fn median_char_width(&self) -> Option<f32> {
        let mut w: Vec<f32> = self
            .words
            .iter()
            .filter(|w| w.text.chars().count() >= 3 && w.w > 0.0)
            .map(|w| w.w / w.text.chars().count() as f32)
            .collect();
        if w.len() < 20 {
            return None;
        }
        w.sort_by(|a, b| a.total_cmp(b));
        Some(w[w.len() / 2])
    }
}

/// PNG bytes of `img`, for tests.
#[cfg(test)]
pub(crate) fn test_png(img: &RgbImage) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn png(w: u32, h: u32, px: [u8; 3]) -> Vec<u8> {
        test_png(&RgbImage::from_pixel(w, h, image::Rgb(px)))
    }

    #[test]
    fn parses_words_targets_and_image() {
        let v = json!({
            "url": "https://a.org/", "title": "A", "vw": 80, "vh": 32, "sy": 5.0, "ph": 900.0,
            "words": [
                {"t": "Hola", "x": 8.0, "y": 0.0, "w": 30.0, "h": 16.0, "c": [255, 0, 0, 1], "b": true},
                {"t": "", "x": 0, "y": 0, "w": 0, "h": 0, "c": [0,0,0], "b": false}
            ],
            "targets": [{"k": "link", "x": 8.0, "y": 0.0, "w": 30.0, "h": 16.0, "l": "Hola", "h2": "https://a.org/x"},
                        {"k": "weird", "x": 0, "y": 0, "w": 4, "h": 4, "l": "", "h2": null}]
        });
        let s = Snapshot::from_json(&v, &png(80, 32, [1, 2, 3])).unwrap();
        assert_eq!(
            (s.url.as_str(), s.title.as_str(), s.viewport),
            ("https://a.org/", "A", (80, 32))
        );
        assert_eq!(s.words.len(), 1, "empty words are dropped");
        assert_eq!(s.words[0].fg, Rgb(255, 0, 0));
        assert!(s.words[0].bold);
        assert_eq!(s.targets[0].kind, Kind::Link);
        assert_eq!(s.targets[0].href.as_deref(), Some("https://a.org/x"));
        assert_eq!(s.targets[1].kind, Kind::Other);
        assert_eq!(s.targets[0].center(), (23.0, 8.0));
        assert_eq!(s.image.dimensions(), (80, 32));
        assert_eq!((s.scroll_y, s.page_height), (5.0, 900.0));
    }

    #[test]
    fn rejects_a_broken_png() {
        assert!(Snapshot::from_json(&json!({}), b"nope").is_err());
    }

    #[test]
    fn median_char_width_needs_enough_words() {
        let mut s =
            Snapshot::from_json(&json!({"vw": 10, "vh": 10}), &png(10, 10, [0; 3])).unwrap();
        assert_eq!(s.median_char_width(), None);
        for _ in 0..30 {
            s.words.push(Word {
                text: "abcd".into(),
                x: 0.0,
                y: 0.0,
                w: 30.0,
                h: 16.0,
                fg: Rgb::default(),
                bold: false,
            });
        }
        assert_eq!(s.median_char_width(), Some(7.5));
    }
}
