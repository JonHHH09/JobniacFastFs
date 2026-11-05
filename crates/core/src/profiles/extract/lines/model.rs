//! The line model: one line of document text with optional page geometry.

use jobniac_text::str_rules::{split_lines, strip_text};

/// The position and typography of one line on its page.
///
/// Coordinates are integer centipoints (1/100 pt) measured from the page's top-left corner, so
/// `y` grows downward. `x1` and `y1` are the right and bottom edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineGeometry {
    /// Left edge.
    pub x0: i32,
    /// Top edge.
    pub y0: i32,
    /// Right edge.
    pub x1: i32,
    /// Bottom edge.
    pub y1: i32,
    /// Font size in centipoints.
    pub font_size_centipoints: i32,
    /// Whether the line is set in a bold face.
    pub bold: bool,
    /// Whether the line is set in an italic face.
    pub italic: bool,
    /// Width of the page the line is on.
    pub page_width_centipoints: i32,
    /// Height of the page the line is on.
    pub page_height_centipoints: i32,
}

impl LineGeometry {
    /// The width of the line box.
    #[must_use]
    pub fn width(&self) -> i32 {
        self.x1.saturating_sub(self.x0)
    }

    /// The height of the line box.
    #[must_use]
    pub fn height(&self) -> i32 {
        self.y1.saturating_sub(self.y0)
    }
}

/// One line of text of a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextLine {
    /// Zero-based page number. Plain text is page 0.
    pub page: u32,
    /// Position of the line in the document, in reading order (set by [`super::LineDocument`]).
    pub index: u32,
    /// The stripped, non-empty text of the line.
    pub text: String,
    /// The page geometry, when the source had it.
    pub geometry: Option<LineGeometry>,
    /// Whether a blank line (plain text) or a paragraph-sized gap preceded the line. For geometry
    /// lines `read_lines` derives it from the vertical gap to the previous line of the same page and
    /// column (more than 0.4 of the font size), so plain and geometry inputs share one meaning.
    pub blank_before: bool,
}

impl TextLine {
    /// A line with no blank line before it.
    #[must_use]
    pub fn new(page: u32, index: u32, text: &str, geometry: Option<LineGeometry>) -> Self {
        Self {
            page,
            index,
            text: strip_text(text).to_owned(),
            geometry,
            blank_before: false,
        }
    }
}

/// Reads plain text into lines on page 0, without geometry.
///
/// Lines are split with `split_lines` and stripped with `strip_text`, as the section split does.
/// Blank lines are dropped but remembered in [`TextLine::blank_before`].
#[must_use]
pub fn lines_from_text(text: &str) -> Vec<TextLine> {
    let mut lines = Vec::new();
    let mut blank = false;
    for raw in split_lines(text) {
        let line = strip_text(raw);
        if line.is_empty() {
            blank = true;
            continue;
        }
        let index = u32::try_from(lines.len()).unwrap_or(u32::MAX);
        let mut item = TextLine::new(0, index, line, None);
        item.blank_before = blank && !lines.is_empty();
        lines.push(item);
        blank = false;
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_keeps_blank_line_flags() {
        let lines = lines_from_text("\n  Ada  \nline two\n\n\nline four\r\n");
        let seen: Vec<(&str, bool, u32)> = lines
            .iter()
            .map(|line| (line.text.as_str(), line.blank_before, line.index))
            .collect();
        assert_eq!(
            seen,
            vec![
                ("Ada", false, 0),
                ("line two", false, 1),
                ("line four", true, 2)
            ]
        );
    }
}
