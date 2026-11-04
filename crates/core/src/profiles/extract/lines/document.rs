//! A document as ordered lines: the input of the feature vectors.

use super::locale::{LocaleHint, detect_locale};
use super::model::{TextLine, lines_from_text};
use super::order::read_lines;

/// The lines of one document in reading order, the column of each, and the document locale hint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineDocument {
    /// The lines in reading order.
    pub lines: Vec<TextLine>,
    /// The column of each line: 0 single or spanning, 1 left, 2 right.
    pub columns: Vec<u8>,
    /// The locale read from the whole text.
    pub locale: LocaleHint,
}

impl LineDocument {
    /// A document from plain text: no geometry, one page.
    #[must_use]
    pub fn from_text(text: &str) -> Self {
        Self::from_lines(lines_from_text(text))
    }

    /// A document from lines with or without geometry, in any order: the lines are put in
    /// reading order and wrapped fragments are joined (see [`super::order`]).
    #[must_use]
    pub fn from_lines(lines: Vec<TextLine>) -> Self {
        let ordered = read_lines(lines);
        let text = ordered
            .lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        Self {
            locale: detect_locale(&text),
            lines: ordered.lines,
            columns: ordered.columns,
        }
    }

    /// The text of the lines joined with newlines.
    #[must_use]
    pub fn text(&self) -> String {
        self.lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }
}
