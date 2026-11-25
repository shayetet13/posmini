//! Minimal ESC/POS builder for 58/80 mm thermal printers, with Thai (TIS-620) text.

const ESC: u8 = 0x1B;
const GS: u8 = 0x1D;
const LF: u8 = 0x0A;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left = 0,
    Center = 1,
}

/// Thai vowels/tone marks that sit on the previous letter and take no width.
pub fn is_zero_width(c: char) -> bool {
    matches!(c, '\u{0E31}' | '\u{0E34}'..='\u{0E3A}' | '\u{0E47}'..='\u{0E4E}')
}

pub fn display_width(text: &str) -> usize {
    text.chars().filter(|c| !is_zero_width(*c)).count()
}

/// TIS-620: ASCII as is, Thai block U+0E01..U+0E5B -> 0xA1..0xFB, anything else -> '?'.
pub fn tis620(text: &str) -> Vec<u8> {
    text.chars()
        .map(|c| match c {
            '\u{0}'..='\u{7F}' => c as u8,
            '\u{0E01}'..='\u{0E5B}' => (c as u32 - 0x0E01 + 0xA1) as u8,
            _ => b'?',
        })
        .collect()
}

/// Splits text into lines of at most `width` columns. Breaks at spaces when possible and
/// never puts a zero-width mark at the start of a line.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut used = 0;
    for word in text.split_whitespace() {
        let word_width = display_width(word);
        let separator = usize::from(used > 0);
        if used + separator + word_width <= width {
            if separator == 1 {
                current.push(' ');
            }
            current.push_str(word);
            used += separator + word_width;
            continue;
        }
        if used > 0 {
            lines.push(std::mem::take(&mut current));
            used = 0;
        }
        for c in word.chars() {
            let w = usize::from(!is_zero_width(c));
            if used + w > width {
                lines.push(std::mem::take(&mut current));
                used = 0;
            }
            current.push(c);
            used += w;
        }
    }
    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
    }
    lines
}

pub struct Builder {
    buf: Vec<u8>,
    cols: usize,
}

impl Builder {
    /// Starts a job: reset, select the Thai code page, left aligned, normal size.
    pub fn new(cols: usize, codepage: u8) -> Self {
        let mut builder = Self {
            buf: Vec::with_capacity(512),
            cols,
        };
        builder.buf.extend([ESC, b'@', ESC, b't', codepage]);
        builder
    }

    pub fn align(&mut self, align: Align) -> &mut Self {
        self.buf.extend([ESC, b'a', align as u8]);
        self
    }

    pub fn bold(&mut self, on: bool) -> &mut Self {
        self.buf.extend([ESC, b'E', u8::from(on)]);
        self
    }

    /// Character size multiplier, 1 (normal) to 8, for width and height.
    pub fn size(&mut self, width: u8, height: u8) -> &mut Self {
        let w = width.clamp(1, 8) - 1;
        let h = height.clamp(1, 8) - 1;
        self.buf.extend([GS, b'!', (w << 4) | h]);
        self
    }

    pub fn normal(&mut self) -> &mut Self {
        self.size(1, 1).bold(false).align(Align::Left)
    }

    /// Bytes that are already printer commands (e.g. a raster picture).
    pub fn raw_bytes(&mut self, bytes: &[u8]) -> &mut Self {
        self.buf.extend(bytes);
        self
    }

    pub fn raw_text(&mut self, text: &str) -> &mut Self {
        self.buf.extend(tis620(text));
        self
    }

    /// One printed line (text is wrapped to the paper width at the current size multiplier 1).
    pub fn line(&mut self, text: &str) -> &mut Self {
        for part in wrap(text, self.cols) {
            self.raw_text(&part);
            self.buf.push(LF);
        }
        self
    }

    /// Like `line`, with an explicit column budget (use `cols / width_multiplier` for big text).
    pub fn line_in(&mut self, text: &str, cols: usize) -> &mut Self {
        for part in wrap(text, cols) {
            self.raw_text(&part);
            self.buf.push(LF);
        }
        self
    }

    pub fn rule(&mut self) -> &mut Self {
        let rule = "-".repeat(self.cols);
        self.raw_text(&rule);
        self.buf.push(LF);
        self
    }

    /// Left text and right-aligned text on one row; the left side wraps if it is too long.
    pub fn row(&mut self, left: &str, right: &str) -> &mut Self {
        let right_width = display_width(right);
        let room = self.cols.saturating_sub(right_width + 1).max(1);
        let parts = wrap(left, room);
        let last = parts.len() - 1;
        for (i, part) in parts.iter().enumerate() {
            self.raw_text(part);
            if i == last {
                let pad = self
                    .cols
                    .saturating_sub(display_width(part) + right_width)
                    .max(1);
                self.raw_text(&" ".repeat(pad));
                self.raw_text(right);
            }
            self.buf.push(LF);
        }
        self
    }

    pub fn feed(&mut self, lines: u8) -> &mut Self {
        self.buf.extend([ESC, b'd', lines]);
        self
    }

    /// Feeds exactly to the cutter (nothing extra, so no wasted paper) and makes a partial cut.
    pub fn cut(&mut self) -> &mut Self {
        self.buf.extend([GS, b'V', 66, 0]);
        self
    }

    pub fn finish(self) -> Vec<u8> {
        self.buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_thai_as_tis620() {
        assert_eq!(tis620("ก"), vec![0xA1]);
        assert_eq!(tis620("ฮ"), vec![0xCE]);
        assert_eq!(tis620("฿"), vec![0xDF]);
        assert_eq!(tis620("เ"), vec![0xE0]);
        assert_eq!(tis620("๙"), vec![0xF9]);
        assert_eq!(tis620("A1 -"), b"A1 -".to_vec());
        assert_eq!(tis620("€"), b"?".to_vec());
    }

    #[test]
    fn marks_take_no_width() {
        assert_eq!(display_width("กา"), 2);
        assert_eq!(display_width("ที่"), 1);
        assert_eq!(display_width("ปาท่องโก๋"), 7);
    }

    #[test]
    fn wraps_by_display_width_without_orphan_marks() {
        let lines = wrap("ปาท่องโก๋สังขยา", 6);
        assert!(lines.iter().all(|l| display_width(l) <= 6));
        assert!(
            lines
                .iter()
                .all(|l| !l.chars().next().is_some_and(is_zero_width))
        );
        assert_eq!(lines.concat(), "ปาท่องโก๋สังขยา");
        assert_eq!(wrap("a bb ccc", 4), vec!["a bb", "ccc"]);
        assert_eq!(wrap("", 10), vec![String::new()]);
    }

    #[test]
    fn row_right_aligns_to_full_width() {
        let mut b = Builder::new(20, 26);
        b.row("รวม", "80.00");
        let bytes = b.finish();
        let body = &bytes[5..bytes.len() - 1];
        assert_eq!(body.len(), 20);
        assert!(body.ends_with(b"80.00"));
    }

    #[test]
    fn job_starts_with_reset_and_code_page_and_cuts() {
        let mut b = Builder::new(32, 26);
        b.line("hi").cut();
        let bytes = b.finish();
        assert_eq!(&bytes[..5], &[0x1B, b'@', 0x1B, b't', 26]);
        assert!(bytes.ends_with(&[0x1D, b'V', 66, 0]));
    }
}
