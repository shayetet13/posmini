//! Draws a bill (`Doc`) as a black-and-white picture, so Thai prints correctly on any ESC/POS
//! printer whatever code pages it has built in. Text is shaped with rustybuzz (Thai tone marks
//! and vowels stack properly) and drawn with a Thai font found on this computer.

use std::path::{Path, PathBuf};

use ab_glyph::{Font as _, FontRef, Glyph as OutlineGlyph, GlyphId, PxScale, point};
use anyhow::{Context, Result, bail};
use rustybuzz::{Face, UnicodeBuffer};

use crate::{
    bill::{Doc, Line, Size},
    escpos::{Align, is_zero_width},
};

const GS: u8 = 0x1D;

const NORMAL_PX: f32 = 24.0;
const TITLE_PX: f32 = 34.0;
const LARGE_PX: f32 = 30.0;
const HUGE_PX: f32 = 120.0;
const MIN_TITLE_PX: f32 = 22.0;
const MIN_HUGE_PX: f32 = 40.0;
const MARGIN_PX: usize = 4;
const MIN_CONTENT_DOTS: usize = 128;
const GAP_PX: usize = 14;
const RULE_PX: usize = 16;
const COLUMN_GAP_PX: f32 = 12.0;
const MAX_RIGHT_SHARE: f32 = 0.6;
/// Anti-aliased coverage (0-255) from which a dot is printed.
const INK_THRESHOLD: u8 = 100;
/// Rows per `GS v 0` command; small bands keep cheap printers' buffers from overflowing.
const BAND_ROWS: usize = 64;
const DASH_ON: usize = 6;
const DASH_OFF: usize = 4;

/// Regular/bold pairs searched in this order (Windows first, then common Linux locations).
const FONT_PAIRS: [(&str, Option<&str>); 6] = [
    (
        r"C:\Windows\Fonts\LeelawUI.ttf",
        Some(r"C:\Windows\Fonts\LeelaUIb.ttf"),
    ),
    (
        r"C:\Windows\Fonts\tahoma.ttf",
        Some(r"C:\Windows\Fonts\tahomabd.ttf"),
    ),
    (
        "/usr/share/fonts/truetype/noto/NotoSansThai-Regular.ttf",
        Some("/usr/share/fonts/truetype/noto/NotoSansThai-Bold.ttf"),
    ),
    (
        "/usr/share/fonts/opentype/noto/NotoSansThai-Regular.otf",
        Some("/usr/share/fonts/opentype/noto/NotoSansThai-Bold.otf"),
    ),
    (
        "/usr/share/fonts/truetype/tlwg/Loma.ttf",
        Some("/usr/share/fonts/truetype/tlwg/Loma-Bold.ttf"),
    ),
    (
        "/usr/share/fonts/truetype/tlwg/Garuda.ttf",
        Some("/usr/share/fonts/truetype/tlwg/Garuda-Bold.ttf"),
    ),
];

/// A black-and-white picture: one byte per dot, 1 = ink.
pub struct Bitmap {
    pub width: usize,
    pub height: usize,
    ink: Vec<u8>,
}

impl Bitmap {
    /// The same picture with blank dots added on the left/right (moves it across the paper).
    fn padded(self, left: usize, right: usize) -> Self {
        if left == 0 && right == 0 {
            return self;
        }
        let width = self.width + left + right;
        let mut ink = Vec::with_capacity(width * self.height);
        for row in self.ink.chunks(self.width) {
            ink.extend(std::iter::repeat_n(0, left));
            ink.extend(row);
            ink.extend(std::iter::repeat_n(0, right));
        }
        Self {
            width,
            height: self.height,
            ink,
        }
    }

    /// `GS v 0` raster commands, in bands, ready to send after a reset.
    pub fn to_escpos(&self) -> Vec<u8> {
        let row_bytes = self.width.div_ceil(8);
        let mut out = Vec::with_capacity(row_bytes * self.height + 64);
        for band in self.ink.chunks(self.width * BAND_ROWS) {
            let rows = band.len() / self.width;
            out.extend([GS, b'v', b'0', 0]);
            out.extend(u16::try_from(row_bytes).unwrap_or(u16::MAX).to_le_bytes());
            out.extend(u16::try_from(rows).unwrap_or(u16::MAX).to_le_bytes());
            for row in band.chunks(self.width) {
                out.extend(pack_row(row, row_bytes));
            }
        }
        out
    }

    /// A 1-bit BMP of exactly the dots that will be printed (used by the settings preview).
    pub fn to_bmp(&self) -> Vec<u8> {
        const HEADERS: usize = 14 + 40 + 8;
        let row_bytes = self.width.div_ceil(8);
        let stride = row_bytes.div_ceil(4) * 4;
        let data_len = stride * self.height;
        let mut out = Vec::with_capacity(HEADERS + data_len);
        let le32 = |v: usize| u32::try_from(v).unwrap_or(u32::MAX).to_le_bytes();
        out.extend(b"BM");
        out.extend(le32(HEADERS + data_len));
        out.extend([0; 4]);
        out.extend(le32(HEADERS));
        out.extend(le32(40));
        out.extend(le32(self.width));
        out.extend(le32(self.height));
        out.extend([1, 0, 1, 0]);
        out.extend([0; 4]);
        out.extend(le32(data_len));
        out.extend([0x13, 0x0B, 0, 0, 0x13, 0x0B, 0, 0]);
        out.extend(le32(2));
        out.extend([0; 4]);
        out.extend([255, 255, 255, 0, 0, 0, 0, 0]);
        let padding = vec![0u8; stride - row_bytes];
        for row in self.ink.chunks(self.width).rev() {
            out.extend(pack_row(row, row_bytes));
            out.extend(&padding);
        }
        out
    }
}

fn pack_row(row: &[u8], row_bytes: usize) -> Vec<u8> {
    (0..row_bytes)
        .map(|i| {
            (0..8).fold(0u8, |byte, bit| {
                let dot = row.get(i * 8 + bit).copied().unwrap_or(0);
                byte | (dot << (7 - bit))
            })
        })
        .collect()
}

// ------------------------------------------------------------------- fonts

struct Typeface<'a> {
    shaper: Face<'a>,
    outlines: FontRef<'a>,
    units_per_em: f32,
}

struct Shaped {
    id: u16,
    advance: f32,
    dx: f32,
    dy: f32,
}

impl<'a> Typeface<'a> {
    fn new(data: &'a [u8]) -> Result<Self> {
        let shaper = Face::from_slice(data, 0).context("อ่านไฟล์ฟอนต์ไม่ได้")?;
        let outlines = FontRef::try_from_slice(data).context("อ่านไฟล์ฟอนต์ไม่ได้")?;
        let units_per_em = outlines.units_per_em().unwrap_or(1000.0);
        Ok(Self {
            shaper,
            outlines,
            units_per_em,
        })
    }

    fn shape(&self, text: &str) -> Vec<Shaped> {
        let mut buffer = UnicodeBuffer::new();
        buffer.push_str(text);
        buffer.guess_segment_properties();
        let out = rustybuzz::shape(&self.shaper, &[], buffer);
        out.glyph_infos()
            .iter()
            .zip(out.glyph_positions())
            .map(|(info, pos)| Shaped {
                id: u16::try_from(info.glyph_id).unwrap_or(0),
                advance: pos.x_advance as f32,
                dx: pos.x_offset as f32,
                dy: pos.y_offset as f32,
            })
            .collect()
    }

    fn width(&self, text: &str, px: f32) -> f32 {
        let units: f32 = self.shape(text).iter().map(|g| g.advance).sum();
        units * px / self.units_per_em
    }

    fn ascent(&self, px: f32) -> f32 {
        self.outlines.ascent_unscaled() * px / self.units_per_em
    }

    fn line_height(&self, px: f32) -> usize {
        let units = self.outlines.ascent_unscaled() - self.outlines.descent_unscaled();
        (units * px / self.units_per_em).ceil().max(px) as usize
    }

    fn draw(&self, canvas: &mut Canvas, text: &str, px: f32, x: f32, baseline: f32) {
        let k = px / self.units_per_em;
        let scale = PxScale::from(px * self.outlines.height_unscaled() / self.units_per_em);
        let mut pen = x;
        for glyph in self.shape(text) {
            let at = point(pen + glyph.dx * k, baseline - glyph.dy * k);
            let outlined = self.outlines.outline_glyph(OutlineGlyph {
                id: GlyphId(glyph.id),
                scale,
                position: at,
            });
            if let Some(outlined) = outlined {
                let bounds = outlined.px_bounds();
                let (left, top) = (bounds.min.x.floor() as i32, bounds.min.y.floor() as i32);
                outlined.draw(|gx, gy, coverage| {
                    canvas.ink(left + gx as i32, top + gy as i32, coverage);
                });
            }
            pen += glyph.advance * k;
        }
    }
}

fn read_font(path: &Path) -> Option<Vec<u8>> {
    std::fs::read(path).ok().filter(|data| !data.is_empty())
}

/// The regular (and, if there is one, bold) font file bytes.
fn find_fonts(custom: Option<&Path>) -> Result<(Vec<u8>, Option<Vec<u8>>)> {
    if let Some(path) = custom {
        let regular = read_font(path)
            .with_context(|| format!("อ่านฟอนต์ {} ไม่ได้ (ตั้งค่า PRINT_FONT)", path.display()))?;
        return Ok((regular, None));
    }
    for (regular, bold) in FONT_PAIRS {
        if let Some(data) = read_font(&PathBuf::from(regular)) {
            let bold = bold.and_then(|b| read_font(&PathBuf::from(b)));
            return Ok((data, bold));
        }
    }
    bail!(
        "ไม่พบฟอนต์ภาษาไทยในเครื่องนี้ (เช่น Leelawadee UI หรือ Tahoma) \
         ตั้งค่า PRINT_FONT ใน .env เป็นไฟล์ฟอนต์ .ttf หรือเลือกพิมพ์แบบข้อความแทน"
    )
}

// ------------------------------------------------------------------ canvas

struct Canvas {
    width: usize,
    coverage: Vec<u8>,
}

impl Canvas {
    fn height(&self) -> usize {
        self.coverage.len() / self.width
    }

    /// Adds blank rows at the bottom and returns the first new row.
    fn grow(&mut self, rows: usize) -> usize {
        let top = self.height();
        self.coverage
            .resize(self.coverage.len() + rows * self.width, 0);
        top
    }

    fn ink(&mut self, x: i32, y: i32, coverage: f32) {
        let (Ok(x), Ok(y)) = (usize::try_from(x), usize::try_from(y)) else {
            return;
        };
        if x >= self.width || y >= self.height() {
            return;
        }
        let value = (coverage.clamp(0.0, 1.0) * 255.0) as u8;
        let cell = &mut self.coverage[y * self.width + x];
        *cell = (*cell).max(value);
    }

    fn dashes(&mut self, y: usize) {
        for row in y..y + 2 {
            for x in
                (MARGIN_PX..self.width - MARGIN_PX).filter(|x| x % (DASH_ON + DASH_OFF) < DASH_ON)
            {
                self.ink(x as i32, row as i32, 1.0);
            }
        }
    }

    fn into_bitmap(self) -> Bitmap {
        let ink = self
            .coverage
            .iter()
            .map(|c| u8::from(*c >= INK_THRESHOLD))
            .collect();
        Bitmap {
            width: self.width,
            height: self.coverage.len() / self.width,
            ink,
        }
    }
}

// ------------------------------------------------------------------ layout

/// Thai vowels written before the consonant they belong to; a line must not end with one.
fn is_leading_vowel(c: char) -> bool {
    matches!(c, 'เ' | 'แ' | 'โ' | 'ใ' | 'ไ')
}

/// Splits a word into pieces that can be put on separate lines: a consonant keeps its tone
/// marks and vowels, and a leading vowel stays with the consonant after it.
fn clusters(word: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut after_leading = false;
    for c in word.chars() {
        if !current.is_empty() && !is_zero_width(c) && !after_leading {
            out.push(std::mem::take(&mut current));
        }
        after_leading = is_leading_vowel(c);
        current.push(c);
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

fn wrap_with(text: &str, max: f32, measure: &dyn Fn(&str) -> f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let candidate = if current.is_empty() {
            word.to_string()
        } else {
            format!("{current} {word}")
        };
        if measure(&candidate) <= max {
            current = candidate;
            continue;
        }
        if !current.is_empty() {
            lines.push(std::mem::take(&mut current));
        }
        if measure(word) <= max {
            current = word.to_string();
            continue;
        }
        for piece in clusters(word) {
            if !current.is_empty() && measure(&format!("{current}{piece}")) > max {
                lines.push(std::mem::take(&mut current));
            }
            current.push_str(&piece);
        }
    }
    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
    }
    lines
}

struct Faces<'a> {
    regular: Typeface<'a>,
    bold: Option<Typeface<'a>>,
}

impl Faces<'_> {
    fn pick(&self, bold: bool) -> &Typeface<'_> {
        match (&self.bold, bold) {
            (Some(face), true) => face,
            _ => &self.regular,
        }
    }

    /// Without a bold font file, bold is faked by drawing twice one dot apart.
    fn draw(&self, canvas: &mut Canvas, bold: bool, text: &str, px: f32, x: f32, baseline: f32) {
        let face = self.pick(bold);
        face.draw(canvas, text, px, x, baseline);
        if bold && self.bold.is_none() {
            face.draw(canvas, text, px, x + 1.0, baseline);
        }
    }

    fn width(&self, bold: bool, text: &str, px: f32) -> f32 {
        let extra = if bold && self.bold.is_none() {
            1.0
        } else {
            0.0
        };
        self.pick(bold).width(text, px) + extra
    }
}

struct Page<'a> {
    faces: Faces<'a>,
    canvas: Canvas,
}

impl Page<'_> {
    fn avail(&self) -> f32 {
        (self.canvas.width - 2 * MARGIN_PX) as f32
    }

    fn text(&mut self, text: &str, size: Size, bold: bool, align: Align) {
        let (px, min_px) = match size {
            Size::Normal => (NORMAL_PX, NORMAL_PX),
            Size::Title => (TITLE_PX, MIN_TITLE_PX),
            Size::Large => (LARGE_PX, LARGE_PX),
            Size::Huge => (HUGE_PX, MIN_HUGE_PX),
        };
        let bold = bold || size != Size::Normal;
        let avail = self.avail();
        let mut px = px;
        while px > min_px && self.faces.width(bold, text, px) > avail {
            px -= 2.0;
        }
        let lines = wrap_with(text, avail, &|t| self.faces.width(bold, t, px));
        for line in lines {
            let x = match align {
                Align::Left => MARGIN_PX as f32,
                Align::Center => {
                    MARGIN_PX as f32 + ((avail - self.faces.width(bold, &line, px)) / 2.0).max(0.0)
                }
            };
            self.line(px, bold, |faces, canvas, baseline| {
                faces.draw(canvas, bold, &line, px, x, baseline);
            });
        }
    }

    fn row(&mut self, left: &str, right: &str, bold: bool) {
        let avail = self.avail();
        let right_width = self.faces.width(bold, right, NORMAL_PX);
        let room = (avail - right_width - COLUMN_GAP_PX).max(avail * (1.0 - MAX_RIGHT_SHARE));
        let lines = wrap_with(left, room, &|t| self.faces.width(bold, t, NORMAL_PX));
        let last = lines.len() - 1;
        for (i, line) in lines.iter().enumerate() {
            let right_x = MARGIN_PX as f32 + avail - right_width;
            self.line(NORMAL_PX, bold, |faces, canvas, baseline| {
                faces.draw(canvas, bold, line, NORMAL_PX, MARGIN_PX as f32, baseline);
                if i == last {
                    faces.draw(canvas, bold, right, NORMAL_PX, right_x, baseline);
                }
            });
        }
    }

    fn line(&mut self, px: f32, bold: bool, paint: impl FnOnce(&Faces, &mut Canvas, f32)) {
        let face = self.faces.pick(bold);
        let (height, ascent) = (face.line_height(px), face.ascent(px));
        let top = self.canvas.grow(height);
        paint(&self.faces, &mut self.canvas, top as f32 + ascent);
    }

    fn rule(&mut self) {
        let top = self.canvas.grow(RULE_PX);
        self.canvas.dashes(top + RULE_PX / 2 - 1);
    }

    fn gap(&mut self) {
        self.canvas.grow(GAP_PX);
    }
}

/// Lays the bill out on a page `width_dots` wide (384 for 58 mm paper, 576 for 80 mm).
/// `shift` moves everything sideways (positive = right) for printers whose paper is not
/// centred on the print head; the text area gets narrower by the same amount.
pub fn render(doc: &Doc, width_dots: usize, shift: i32, font: Option<&Path>) -> Result<Bitmap> {
    let moved = shift.unsigned_abs() as usize;
    let inner = width_dots.saturating_sub(moved).max(MIN_CONTENT_DOTS);
    let (regular, bold) = find_fonts(font)?;
    let faces = Faces {
        regular: Typeface::new(&regular)?,
        bold: bold.as_deref().map(Typeface::new).transpose()?,
    };
    let mut page = Page {
        faces,
        canvas: Canvas {
            width: inner,
            coverage: Vec::new(),
        },
    };
    for line in doc {
        match line {
            Line::Text {
                text,
                size,
                bold,
                align,
            } => page.text(text, *size, *bold, *align),
            Line::Row { left, right, bold } => page.row(left, right, *bold),
            Line::Rule => page.rule(),
        }
    }
    page.gap();
    let (left, right) = if shift >= 0 { (moved, 0) } else { (0, moved) };
    Ok(page.canvas.into_bitmap().padded(left, right))
}

/// 5x7 digits (rows top to bottom, bit 4 = leftmost) for the ruler numbers.
const DIGITS: [[u8; 7]; 10] = [
    [
        0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110,
    ],
    [
        0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
    ],
    [
        0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111,
    ],
    [
        0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110,
    ],
    [
        0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010,
    ],
    [
        0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110,
    ],
    [
        0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110,
    ],
    [
        0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000,
    ],
    [
        0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110,
    ],
    [
        0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100,
    ],
];
const DIGIT_SCALE: usize = 3;
const DIGIT_W: usize = 5 * DIGIT_SCALE;
const DIGIT_GAP: usize = 3;
/// Distance between numbered ruler marks; the last number read times this is the printable width.
pub const RULER_STEP: usize = 48;

/// A page to find out what the printer really prints: a frame at the edges of the page, a
/// ruler numbered every 48 dots (the last number that can still be read, times 48, is the
/// printable width), and a dashed line where the bill's centre will fall.
pub fn calibration(width_dots: usize, shift: i32) -> Bitmap {
    const HEIGHT: usize = 110;
    const FRAME: usize = 3;
    const LABEL_TOP: usize = 34;
    let width = width_dots.max(MIN_CONTENT_DOTS);
    let moved = shift.unsigned_abs() as usize;
    let left = if shift >= 0 { moved } else { 0 };
    let centre = left + width.saturating_sub(moved) / 2;
    let mut ink = vec![0u8; width * HEIGHT];
    for y in 0..HEIGHT {
        for x in 0..width {
            let frame =
                !(FRAME..width - FRAME).contains(&x) || !(FRAME..HEIGHT - FRAME).contains(&y);
            let long = x % RULER_STEP < 2 && y < 28;
            let short = x % (RULER_STEP / 3) < 2 && y < 12;
            let middle = x.abs_diff(centre) < 2 && (y / 8) % 2 == 0;
            ink[y * width + x] = u8::from(frame || long || short || middle);
        }
    }
    // The number sits just left of its mark, so reading it proves the mark itself printed.
    for n in 1..=(width - 1) / RULER_STEP {
        let text = n.to_string();
        let text_w = text.len() * (DIGIT_W + DIGIT_GAP) - DIGIT_GAP;
        let Some(start) = (n * RULER_STEP).checked_sub(text_w + 4) else {
            continue;
        };
        for (i, digit) in text.bytes().map(|b| usize::from(b - b'0')).enumerate() {
            let x0 = start + i * (DIGIT_W + DIGIT_GAP);
            for (row, bits) in DIGITS[digit].iter().enumerate() {
                for col in 0..5 {
                    if bits >> (4 - col) & 1 == 0 {
                        continue;
                    }
                    for dy in 0..DIGIT_SCALE {
                        for dx in 0..DIGIT_SCALE {
                            let (x, y) = (
                                x0 + col * DIGIT_SCALE + dx,
                                LABEL_TOP + row * DIGIT_SCALE + dy,
                            );
                            if x < width && y < HEIGHT {
                                ink[y * width + x] = 1;
                            }
                        }
                    }
                }
            }
        }
    }
    Bitmap {
        width,
        height: HEIGHT,
        ink,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bill::{self, BillSettings, Facts};

    fn chars(text: &str) -> f32 {
        text.chars().filter(|c| !is_zero_width(*c)).count() as f32
    }

    #[test]
    fn clusters_keep_marks_and_leading_vowels_attached() {
        assert_eq!(clusters("ปาท่องโก๋"), vec!["ป", "า", "ท่", "อ", "ง", "โก๋"]);
        assert_eq!(clusters("เก้า"), vec!["เก้", "า"]);
        assert_eq!(clusters("abc"), vec!["a", "b", "c"]);
        assert!(clusters("").is_empty());
    }

    #[test]
    fn wrapping_never_loses_text_or_orphans_marks() {
        let lines = wrap_with("ปาท่องโก๋สังขยา", 6.0, &chars);
        assert!(lines.iter().all(|l| chars(l) <= 6.0));
        assert_eq!(lines.concat(), "ปาท่องโก๋สังขยา");
        assert!(lines.iter().all(|l| !l.starts_with(is_zero_width)));
        assert!(
            lines
                .iter()
                .all(|l| !l.ends_with(|c: char| is_leading_vowel(c)))
        );
        assert_eq!(wrap_with("a bb ccc", 4.0, &chars), vec!["a bb", "ccc"]);
        assert_eq!(wrap_with("", 4.0, &chars), vec![String::new()]);
    }

    #[test]
    fn raster_commands_are_banded_and_sized() {
        let bitmap = Bitmap {
            width: 16,
            height: 130,
            ink: (0..16 * 130).map(|i| u8::from(i % 16 == 0)).collect(),
        };
        let bytes = bitmap.to_escpos();
        // 64 + 64 + 2 rows, each band: 8-byte header + rows * 2 bytes.
        assert_eq!(bytes.len(), 3 * 8 + 130 * 2);
        assert_eq!(&bytes[..8], &[GS, b'v', b'0', 0, 2, 0, 64, 0]);
        assert_eq!(&bytes[8..10], &[0b1000_0000, 0]);
        let tail = 8 + 64 * 2 + 8 + 64 * 2;
        assert_eq!(&bytes[tail..tail + 8], &[GS, b'v', b'0', 0, 2, 0, 2, 0]);
    }

    #[test]
    fn shifting_pads_one_side_and_keeps_the_total_width() {
        let bitmap = Bitmap {
            width: 8,
            height: 2,
            ink: vec![1; 16],
        };
        let right = bitmap.padded(3, 0);
        assert_eq!(right.width, 11);
        assert_eq!(&right.ink[..11], &[0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1]);
        let page = calibration(384, 40);
        assert_eq!(page.width, 384);
        // centre line sits at 40 + (384 - 40) / 2 = 212
        assert_eq!(page.ink[64 * 384 + 212], 1);
        assert_eq!(page.ink[90 * 384 + 300], 0);
        // The ruler numbers exist: some ink in the label band left of the first long mark.
        let band =
            |x0: usize, x1: usize| (34..55).any(|y| (x0..x1).any(|x| page.ink[y * 384 + x] == 1));
        assert!(band(20, 46), "label 1");
        assert!(band(300, 336), "label 7");
    }

    #[test]
    fn bmp_has_header_palette_and_bottom_up_rows() {
        let bitmap = Bitmap {
            width: 8,
            height: 2,
            ink: [vec![1; 8], vec![0; 8]].concat(),
        };
        let bmp = bitmap.to_bmp();
        assert_eq!(&bmp[..2], b"BM");
        assert_eq!(bmp.len(), 62 + 2 * 4);
        assert_eq!(&bmp[62..66], &[0, 0, 0, 0]); // bottom row (blank) first
        assert_eq!(&bmp[66..70], &[0xFF, 0, 0, 0]);
    }

    /// Runs only where a Thai font is installed; checks the bill really gets drawn.
    #[test]
    fn thai_bill_is_drawn_when_a_font_exists() {
        if find_fonts(None).is_err() {
            return;
        }
        let bill = BillSettings::default();
        let order = bill::sample_order(&bill);
        let facts = Facts {
            shop: "ร้านทดสอบ",
            bank: Some("kbank"),
            stamp: "22/11/2569 15:29",
        };
        let doc = bill::ticket(&bill, &facts, &order);
        let bitmap = render(&doc, 384, 0, None).unwrap();
        assert_eq!(bitmap.width, 384);
        assert!(bitmap.height > 300);
        let ink = bitmap.ink.iter().filter(|d| **d == 1).count();
        assert!(ink > 3_000, "ink = {ink}");
        let wide = render(&doc, 576, 0, None).unwrap();
        assert_eq!(wide.width, 576);
        if let Ok(dir) = std::env::var("POSMINI_RASTER_DUMP") {
            let write = |name: &str, bitmap: &Bitmap| {
                let _ = std::fs::write(Path::new(&dir).join(name), bitmap.to_bmp());
            };
            write("ticket58.bmp", &bitmap);
            write("ticket80.bmp", &wide);
            let slip = bill::order_slip(&bill, &facts, &order);
            write("slip58.bmp", &render(&slip, 384, 0, None).unwrap());
        }
    }
}
