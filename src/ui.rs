// A little drawing kit for the settings window and the tray menu: shapes
// with tiny-skia, text with the desktop's fonts and fontdue, all in logical
// pixels scaled to the display's DPI.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use fontdue::{Font, FontSettings, Metrics};
use tiny_skia::{
    Color, FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, PremultipliedColorU8, Stroke, Transform,
};

use crate::geometry::Rect;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgba(pub u8, pub u8, pub u8, pub u8);

impl Rgba {
    pub const fn rgb(hex: u32) -> Rgba {
        Rgba((hex >> 16) as u8, (hex >> 8) as u8, hex as u8, 255)
    }

    pub const fn alpha(self, a: u8) -> Rgba {
        Rgba(self.0, self.1, self.2, a)
    }

    fn color(self) -> Color {
        Color::from_rgba8(self.0, self.1, self.2, self.3)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Weight {
    Regular,
    Bold,
}

// The desktop's sans-serif font, as fontconfig picks it, or a common one.
fn find_font(bold: bool) -> Option<Font> {
    let pattern = if bold { "sans-serif:bold" } else { "sans-serif" };
    let matched = std::process::Command::new("fc-match")
        .args(["-f", "%{file}\n%{index}", pattern])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|out| {
            let (file, index) = out.split_once('\n')?;
            Some((file.to_string(), index.trim().parse().unwrap_or(0)))
        });
    let style = if bold { "Bold" } else { "Regular" };
    let fallbacks = [
        format!("/usr/share/fonts/truetype/noto/NotoSans-{style}.ttf"),
        format!("/usr/share/fonts/noto/NotoSans-{style}.ttf"),
        format!("/usr/share/fonts/google-noto/NotoSans-{style}.ttf"),
        format!("/usr/share/fonts/truetype/dejavu/DejaVuSans{}.ttf", if bold { "-Bold" } else { "" }),
        format!("/usr/share/fonts/TTF/DejaVuSans{}.ttf", if bold { "-Bold" } else { "" }),
        format!("/usr/share/fonts/dejavu/DejaVuSans{}.ttf", if bold { "-Bold" } else { "" }),
        format!("/usr/share/fonts/liberation/LiberationSans-{style}.ttf"),
        format!("/usr/share/fonts/truetype/liberation/LiberationSans-{style}.ttf"),
    ];
    matched.into_iter().chain(fallbacks.into_iter().map(|f| (f, 0))).find_map(|(file, index)| {
        let data = std::fs::read(&file).ok()?;
        Font::from_bytes(data, FontSettings { collection_index: index, ..FontSettings::default() }).ok()
    })
}

type GlyphKey = (char, u32, Weight);

pub struct Fonts {
    regular: Font,
    bold: Font,
    glyphs: RefCell<HashMap<GlyphKey, (Metrics, Vec<u8>)>>,
}

impl Fonts {
    pub fn load() -> Result<Rc<Fonts>, String> {
        let regular = find_font(false).ok_or("No font found: install fontconfig and a sans-serif font.")?;
        let bold = find_font(true).unwrap_or_else(|| regular.clone());
        Ok(Rc::new(Fonts { regular, bold, glyphs: RefCell::new(HashMap::new()) }))
    }

    fn font(&self, weight: Weight) -> &Font {
        match weight {
            Weight::Regular => &self.regular,
            Weight::Bold => &self.bold,
        }
    }

    // The width of `text` at `px` pixels.
    fn width(&self, text: &str, px: f32, weight: Weight) -> f32 {
        let font = self.font(weight);
        let mut width = 0.0;
        let mut previous = None;
        for c in text.chars() {
            if let Some(p) = previous {
                width += font.horizontal_kern(p, c, px).unwrap_or(0.0);
            }
            width += font.metrics(c, px).advance_width;
            previous = Some(c);
        }
        width
    }

    fn ascent(&self, px: f32, weight: Weight) -> f32 {
        self.font(weight).horizontal_line_metrics(px).map_or(px * 0.8, |m| m.ascent)
    }
}

// A pixmap to draw on in logical pixels, from `origin`.
pub struct Canvas {
    pub pixmap: Pixmap,
    pub scale: f32,
    pub fonts: Rc<Fonts>,
    origin: (f32, f32),
}

fn round_rect(r: Rect, radius: f32, scale: f32, origin: (f32, f32)) -> Option<tiny_skia::Path> {
    let (x, y) = ((r.x as f32 + origin.0) * scale, (r.y as f32 + origin.1) * scale);
    let (w, h) = (r.width as f32 * scale, r.height as f32 * scale);
    let k = (radius * scale).min(w / 2.0).min(h / 2.0).max(0.0);
    // Circular arcs, with the usual cubic approximation.
    let c = k * 0.4477;
    let mut pb = PathBuilder::new();
    pb.move_to(x + k, y);
    pb.line_to(x + w - k, y);
    pb.cubic_to(x + w - c, y, x + w, y + c, x + w, y + k);
    pb.line_to(x + w, y + h - k);
    pb.cubic_to(x + w, y + h - c, x + w - c, y + h, x + w - k, y + h);
    pb.line_to(x + k, y + h);
    pb.cubic_to(x + c, y + h, x, y + h - c, x, y + h - k);
    pb.line_to(x, y + k);
    pb.cubic_to(x, y + c, x + c, y, x + k, y);
    pb.close();
    pb.finish()
}

fn paint(color: Rgba) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color(color.color());
    paint.anti_alias = true;
    paint
}

impl Canvas {
    // `width` and `height` in logical pixels.
    pub fn new(width: f64, height: f64, scale: f32, fonts: Rc<Fonts>) -> Canvas {
        let w = ((width as f32 * scale).ceil() as u32).max(1);
        let h = ((height as f32 * scale).ceil() as u32).max(1);
        Canvas { pixmap: Pixmap::new(w, h).expect("a non-empty size"), scale, fonts, origin: (0.0, 0.0) }
    }

    pub fn set_origin(&mut self, x: f64, y: f64) {
        self.origin = (x as f32, y as f32);
    }

    pub fn clear(&mut self, color: Rgba) {
        self.pixmap.fill(color.color());
    }

    pub fn fill_rect(&mut self, r: Rect, radius: f32, color: Rgba) {
        if let Some(path) = round_rect(r, radius, self.scale, self.origin) {
            self.pixmap.fill_path(&path, &paint(color), FillRule::Winding, Transform::identity(), None);
        }
    }

    // A border inside `r`.
    pub fn stroke_rect(&mut self, r: Rect, radius: f32, width: f32, color: Rgba) {
        let half = width as f64 / 2.0;
        let inner = Rect::new(r.x + half, r.y + half, r.width - 2.0 * half, r.height - 2.0 * half);
        if let Some(path) = round_rect(inner, radius - width / 2.0, self.scale, self.origin) {
            let stroke = Stroke { width: width * self.scale, ..Stroke::default() };
            self.pixmap.stroke_path(&path, &paint(color), &stroke, Transform::identity(), None);
        }
    }

    pub fn fill_circle(&mut self, x: f64, y: f64, radius: f64, color: Rgba) {
        self.fill_rect(Rect::new(x - radius, y - radius, 2.0 * radius, 2.0 * radius), radius as f32, color);
    }

    // Strokes the lines and curves `build` adds, in logical pixels from
    // (x, y).
    pub fn stroke(&mut self, x: f64, y: f64, width: f32, color: Rgba, build: impl FnOnce(&mut PathBuilder)) {
        let mut pb = PathBuilder::new();
        build(&mut pb);
        let Some(path) = pb.finish() else { return };
        let stroke = Stroke { width, line_cap: LineCap::Round, line_join: LineJoin::Round, ..Stroke::default() };
        self.pixmap.stroke_path(&path, &paint(color), &stroke, self.transform(x, y), None);
    }

    pub fn fill(&mut self, x: f64, y: f64, color: Rgba, build: impl FnOnce(&mut PathBuilder)) {
        let mut pb = PathBuilder::new();
        build(&mut pb);
        let Some(path) = pb.finish() else { return };
        self.pixmap.fill_path(&path, &paint(color), FillRule::Winding, self.transform(x, y), None);
    }

    fn transform(&self, x: f64, y: f64) -> Transform {
        Transform::from_translate(x as f32 + self.origin.0, y as f32 + self.origin.1).post_scale(self.scale, self.scale)
    }

    pub fn draw_pixmap(&mut self, x: f64, y: f64, pixmap: &Pixmap) {
        let (x, y) = ((x as f32 + self.origin.0) * self.scale, (y as f32 + self.origin.1) * self.scale);
        self.pixmap.draw_pixmap(x.round() as i32, y.round() as i32, pixmap.as_ref(), &Default::default(),
                                Transform::identity(), None);
    }

    pub fn text_width(&self, text: &str, size: f32, weight: Weight) -> f64 {
        (self.fonts.width(text, size * self.scale, weight) / self.scale) as f64
    }

    // Draws `text` with its top at `y`, and gives its width.
    pub fn text(&mut self, x: f64, y: f64, text: &str, size: f32, weight: Weight, color: Rgba) -> f64 {
        let px = size * self.scale;
        let fonts = self.fonts.clone();
        let font = fonts.font(weight);
        let baseline = ((y as f32 + self.origin.1) * self.scale + fonts.ascent(px, weight)).round();
        let mut pen = (x as f32 + self.origin.0) * self.scale;
        let mut previous = None;
        let mut glyphs = fonts.glyphs.borrow_mut();
        for c in text.chars() {
            if let Some(p) = previous {
                pen += font.horizontal_kern(p, c, px).unwrap_or(0.0);
            }
            previous = Some(c);
            let (metrics, coverage) =
                glyphs.entry((c, px.to_bits(), weight)).or_insert_with(|| font.rasterize(c, px));
            let left = (pen + metrics.xmin as f32).round() as i32;
            let top = baseline as i32 - metrics.ymin - metrics.height as i32;
            self.blend(left, top, metrics.width, coverage, color);
            pen += metrics.advance_width;
        }
        ((pen / self.scale - self.origin.0) as f64 - x).max(0.0)
    }

    // Draws `text` in `color` where the glyph's coverage says so.
    fn blend(&mut self, left: i32, top: i32, width: usize, coverage: &[u8], color: Rgba) {
        if width == 0 {
            return;
        }
        let (pw, ph) = (self.pixmap.width() as i32, self.pixmap.height() as i32);
        let pixels = self.pixmap.pixels_mut();
        for (row, line) in coverage.chunks(width).enumerate() {
            let y = top + row as i32;
            if y < 0 || y >= ph {
                continue;
            }
            for (col, &c) in line.iter().enumerate() {
                let x = left + col as i32;
                if c == 0 || x < 0 || x >= pw {
                    continue;
                }
                let a = c as u32 * color.3 as u32 / 255;
                let pixel = &mut pixels[(y * pw + x) as usize];
                let mix = |src: u8, dst: u8| ((src as u32 * a + dst as u32 * (255 - a)) / 255) as u8;
                let blended = PremultipliedColorU8::from_rgba(
                    mix(color.0, pixel.red()),
                    mix(color.1, pixel.green()),
                    mix(color.2, pixel.blue()),
                    (a + pixel.alpha() as u32 * (255 - a) / 255) as u8,
                );
                if let Some(blended) = blended {
                    *pixel = blended;
                }
            }
        }
    }

    // Splits `text` into lines no wider than `width`.
    pub fn wrap(&self, text: &str, size: f32, weight: Weight, width: f64) -> Vec<String> {
        let mut lines = Vec::new();
        for paragraph in text.split('\n') {
            let mut line = String::new();
            for word in paragraph.split(' ') {
                let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
                if self.text_width(&candidate, size, weight) <= width {
                    line = candidate;
                    continue;
                }
                if !line.is_empty() {
                    lines.push(std::mem::take(&mut line));
                }
                // Words too long for a line (paths) break anywhere.
                for c in word.chars() {
                    line.push(c);
                    if line.chars().count() > 1 && self.text_width(&line, size, weight) > width {
                        line.pop();
                        lines.push(std::mem::replace(&mut line, c.to_string()));
                    }
                }
            }
            lines.push(line);
        }
        lines
    }

    // The pixels as the X server wants them for a 24- or 32-bit TrueColor
    // visual: BGRA, little-endian. Premultiplied, as compositors expect.
    pub fn bgra(&self) -> Vec<u8> {
        let mut data = Vec::with_capacity(self.pixmap.data().len());
        for p in self.pixmap.pixels() {
            data.extend_from_slice(&[p.blue(), p.green(), p.red(), p.alpha()]);
        }
        data
    }
}
