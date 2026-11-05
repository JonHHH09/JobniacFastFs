//! The parent side of the `pdf-layout` worker operation: positioned line fragments of a PDF with
//! font information, for classifying resume and cover-letter lines by layout.
//!
//! The operation runs in the CLI binary re-executed as `__worker pdf-layout`, under the limits and
//! sandbox profile of text extraction. This module owns everything that does not need PDFium: the
//! grouping of characters into fragments, the response codec and its validation, and the
//! parent-side call. The request framing is that of [`super::pdf`] (operation byte `4`).
//!
//! Response body: `{"result": {...}}` with `pageCount`, `pages` and `truncated`, or
//! `{"error": "page_limit"}`. A page is `{"page", "width", "height", "lines"}` (centipoints) and a
//! line is `{"text", "x0", "y0", "x1", "y1", "size", "bold", "italic"}` with integer centipoints,
//! the origin at the top-left of the page.
//!
//! Grouping rule (see [`group_page_chars`]): characters are walked in content order. A character
//! continues the open fragment while its baseline is within [`BASELINE_TOLERANCE_EM`] times the
//! font size of the open fragment's baseline and the horizontal gap from the previous character is
//! below [`GAP_LIMIT_EM`] times the font size (and not further back than [`OVERLAP_LIMIT_EM`]
//! times it), and its bold and italic flags equal the fragment's. Otherwise it starts a new
//! fragment, so two columns on one baseline give two fragments, and so does a bold label in front
//! of a regular value.

use std::future::Future;
use std::path::Path;

use jobniac_core::documents::model::{
    LAYOUT_BOUNDS_TOLERANCE, LayoutLine, MAX_LAYOUT_FONT_SIZE, MAX_LAYOUT_LINE_CHARS,
    MAX_LAYOUT_LINES, PageLayout, PdfLayout,
};
use jobniac_text::str_rules::is_text_whitespace;
use serde_json::{Map, Value, json};

use crate::workers::kinds::WorkerKind;
use crate::workers::pdf::{
    MAX_CHARACTERS_LIMIT, MAX_FILE_BYTES, MAX_PAGE_COUNT, PdfError, PdfOperation, encode_request,
    map_worker_error, validate_pdf_content,
};
use crate::workers::supervisor::WorkerSupervisor;

/// How far a baseline may differ from the open fragment's, in font sizes.
pub const BASELINE_TOLERANCE_EM: f32 = 0.5;

/// The horizontal gap, in font sizes, at which a new fragment starts.
pub const GAP_LIMIT_EM: f32 = 1.5;

/// How far back, in font sizes, a character may start before the previous one ended and still
/// continue the fragment (overprinted or kerned glyphs).
pub const OVERLAP_LIMIT_EM: f32 = 1.0;

/// The horizontal gap, in font sizes, above which a space is inserted when the text has none.
pub const WORD_GAP_EM: f32 = 0.3;

/// One character of a page as PDFium reports it, in PDF points with the origin at the bottom-left
/// of the page.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutChar {
    /// The character.
    pub character: char,
    /// Whether PDFium synthesized it (spaces and line breaks it adds between runs).
    pub generated: bool,
    /// The left edge of its box.
    pub left: f32,
    /// The right edge of its box.
    pub right: f32,
    /// The bottom edge of its box.
    pub bottom: f32,
    /// The top edge of its box.
    pub top: f32,
    /// The vertical position of its baseline.
    pub baseline: f32,
    /// The effective font size.
    pub font_size: f32,
    /// Whether the font is bold.
    pub bold: bool,
    /// Whether the font is italic.
    pub italic: bool,
}

/// What is left of the document bounds while pages are grouped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutBudget {
    /// How many more fragments may be returned.
    pub lines_left: usize,
    /// How many more characters may be returned.
    pub chars_left: usize,
    /// Whether a bound cut output.
    pub truncated: bool,
    /// Whether the document budget is spent and no further fragment can be returned.
    pub stopped: bool,
}

impl LayoutBudget {
    /// The full document budget: [`MAX_LAYOUT_LINES`] fragments and [`MAX_CHARACTERS_LIMIT`]
    /// characters.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            lines_left: MAX_LAYOUT_LINES,
            chars_left: MAX_CHARACTERS_LIMIT,
            truncated: false,
            stopped: false,
        }
    }
}

impl Default for LayoutBudget {
    fn default() -> Self {
        Self::new()
    }
}

/// Whether a font is bold: a weight of 600 or more, PDFium's bold reinforcement, or a font name
/// that says so (`Bold`, `Black`, `Heavy`, `Semibold`, case-insensitive).
#[must_use]
pub fn is_bold_font(weight: Option<u32>, reinforced: bool, font_name: &str) -> bool {
    let name = font_name.to_ascii_lowercase();
    weight.is_some_and(|value| value >= 600)
        || reinforced
        || ["bold", "black", "heavy", "semibold"]
            .iter()
            .any(|marker| name.contains(marker))
}

/// The style an embedded font program declares about itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FontStyle {
    /// The `OS/2` weight class (100 to 900), when the program has one.
    pub weight: Option<u32>,
    /// Whether the program declares itself bold (`head.macStyle` or `OS/2.fsSelection`).
    pub bold: bool,
    /// Whether the program declares itself italic.
    pub italic: bool,
}

/// A big-endian `u16` at `offset`.
fn be_u16(data: &[u8], offset: usize) -> Option<u16> {
    let bytes = data.get(offset..offset.checked_add(2)?)?;
    Some(u16::from_be_bytes([bytes[0], bytes[1]]))
}

/// A big-endian `u32` at `offset`.
fn be_u32(data: &[u8], offset: usize) -> Option<u32> {
    let bytes = data.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

/// The weight class from which a font counts as bold (`OS/2.usWeightClass`).
pub const BOLD_WEIGHT_CLASS: u32 = 600;

/// Whether the field of `size` bytes at `field` lies inside a table of `length` bytes.
fn in_table(length: usize, field: usize, size: usize) -> bool {
    field.checked_add(size).is_some_and(|end| end <= length)
}

/// The style declared by an embedded TrueType or OpenType font program, read from its `OS/2` and
/// `head` tables. Many PDF producers name every face of a family alike and leave the font weight
/// out of the descriptor, so this is the only place the weight is recorded. Bold means a weight
/// class of [`BOLD_WEIGHT_CLASS`] or more, the bold bit of `OS/2.fsSelection` or the bold bit of
/// `head.macStyle`.
///
/// `None` for data that is not a single sfnt font (a bare CFF or Type 1 program, a `ttcf`
/// collection), for a table directory that is cut off, and for a font that declares nothing. A
/// field is read only when the table's declared length covers it, and when a table is listed
/// twice the first entry counts.
#[must_use]
pub fn sfnt_style(data: &[u8]) -> Option<FontStyle> {
    const MAX_TABLES: usize = 256;
    let tag = be_u32(data, 0)?;
    if !matches!(tag, 0x0001_0000 | 0x4F54_544F | 0x7472_7565) {
        return None;
    }
    let tables = usize::from(be_u16(data, 4)?).min(MAX_TABLES);
    let mut style = FontStyle::default();
    let (mut seen_os2, mut seen_head, mut known) = (false, false, false);
    for index in 0..tables {
        let record = 12 + index * 16;
        let name = data.get(record..record + 4)?;
        let offset = usize::try_from(be_u32(data, record + 8)?).ok()?;
        let length = usize::try_from(be_u32(data, record + 12)?).ok()?;
        let field = |at: usize| be_u16(data, offset.checked_add(at)?);
        match name {
            b"OS/2" if !seen_os2 => {
                seen_os2 = true;
                if let Some(weight) = in_table(length, 4, 2).then(|| field(4)).flatten() {
                    style.weight =
                        Some(u32::from(weight)).filter(|value| (1..=1000).contains(value));
                    style.bold |= style.weight.is_some_and(|value| value >= BOLD_WEIGHT_CLASS);
                    known = true;
                }
                if let Some(selection) = in_table(length, 62, 2).then(|| field(62)).flatten() {
                    style.bold |= selection & 0x20 != 0;
                    style.italic |= selection & 0x01 != 0;
                }
            }
            b"head" if !seen_head => {
                seen_head = true;
                if let Some(mac_style) = in_table(length, 44, 2).then(|| field(44)).flatten() {
                    style.bold |= mac_style & 0x01 != 0;
                    style.italic |= mac_style & 0x02 != 0;
                    known = true;
                }
            }
            _ => {}
        }
    }
    known.then_some(style)
}

/// Points as centipoints, rounded half away from zero and clamped into `0..=maximum`. NaN is 0.
#[must_use]
pub fn centipoints_within(points: f32, maximum: u32) -> u32 {
    let scaled = (f64::from(points) * 100.0).round();
    if scaled.is_nan() || scaled <= 0.0 {
        return 0;
    }
    // The float-to-integer cast saturates; the clamp keeps the value within the page.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let value = scaled as u32;
    value.min(maximum)
}

/// A fragment while it is being grown.
struct Fragment {
    text: String,
    text_chars: usize,
    left: f32,
    right: f32,
    bottom: f32,
    top: f32,
    sizes: Vec<u32>,
    bold: bool,
    italic: bool,
    baseline: f32,
    cursor_right: f32,
    cursor_size: f32,
    pending_space: bool,
    full: bool,
}

impl Fragment {
    fn start(character: &LayoutChar) -> Self {
        Self {
            text: String::new(),
            text_chars: 0,
            left: character.left,
            right: character.right,
            bottom: character.bottom,
            top: character.top,
            sizes: Vec::new(),
            bold: character.bold,
            italic: character.italic,
            baseline: character.baseline,
            cursor_right: character.right,
            cursor_size: character.font_size,
            pending_space: false,
            full: false,
        }
    }

    /// Whether `character` continues this fragment.
    fn continues_with(&self, character: &LayoutChar) -> bool {
        let size = self.cursor_size.max(character.font_size);
        let gap = character.left - self.cursor_right;
        character.bold == self.bold
            && character.italic == self.italic
            && (character.baseline - self.baseline).abs() <= BASELINE_TOLERANCE_EM * size
            && gap < GAP_LIMIT_EM * size
            && gap >= -OVERLAP_LIMIT_EM * size
    }

    /// Appends `character`, which continues this fragment (or starts it). Returns whether a
    /// per-fragment bound cut it.
    fn push(&mut self, character: &LayoutChar, first: bool) -> bool {
        let size = self.cursor_size.max(character.font_size);
        let gap = character.left - self.cursor_right;
        let space =
            !first && !self.text.is_empty() && (self.pending_space || gap > WORD_GAP_EM * size);
        let mut cut = false;
        if self.full || self.text_chars + usize::from(space) + 1 > MAX_LAYOUT_LINE_CHARS {
            self.full = true;
            cut = true;
        } else {
            if space {
                self.text.push(' ');
                self.text_chars += 1;
            }
            self.text.push(character.character);
            self.text_chars += 1;
            self.left = self.left.min(character.left);
            self.right = self.right.max(character.right);
            self.bottom = self.bottom.min(character.bottom);
            self.top = self.top.max(character.top);
            self.sizes.push(centipoints_within(
                character.font_size,
                MAX_LAYOUT_FONT_SIZE,
            ));
        }
        self.baseline = character.baseline;
        self.cursor_right = character.right;
        self.cursor_size = character.font_size;
        self.pending_space = false;
        cut
    }
}

/// Finishes a fragment against the budget. `None` when it is empty or the budget cannot take it.
fn finish(
    fragment: Fragment,
    width: u32,
    height: u32,
    budget: &mut LayoutBudget,
) -> Option<LayoutLine> {
    let Fragment {
        text,
        mut sizes,
        bold,
        italic,
        left,
        right,
        bottom,
        top,
        ..
    } = fragment;
    let glyphs = sizes.len();
    if text.is_empty() || glyphs == 0 {
        return None;
    }
    if budget.lines_left == 0 || budget.chars_left == 0 {
        budget.truncated = true;
        budget.stopped = true;
        return None;
    }
    let mut text = text;
    if text.chars().count() > budget.chars_left {
        text = text.chars().take(budget.chars_left).collect();
        text.truncate(text.trim_end().len());
        budget.truncated = true;
        budget.stopped = true;
        if text.is_empty() {
            return None;
        }
    }
    budget.lines_left -= 1;
    budget.chars_left -= text.chars().count();
    sizes.sort_unstable();
    let size = sizes[(glyphs - 1) / 2].clamp(1, MAX_LAYOUT_FONT_SIZE);
    Some(LayoutLine {
        text,
        x0: centipoints_within(left, width),
        y0: height - centipoints_within(top, height),
        x1: centipoints_within(right, width),
        y1: height - centipoints_within(bottom, height),
        font_size_centipoints: size,
        bold,
        italic,
    })
}

/// Groups the characters of one page, in content order, into line fragments.
///
/// `width` and `height` are the page size in centipoints; coordinates are clamped into the page and
/// flipped to the top-left origin. Whitespace, PDFium-generated characters and control characters
/// separate words but take no part in the geometry; characters with a non-finite box or a font
/// size that is not positive are ignored. A space is inserted between two characters when the text
/// has one between them or the gap exceeds [`WORD_GAP_EM`] font sizes. The font size of a fragment
/// is the median (lower middle) over its characters. A change of bold or italic starts a new
/// fragment, so the flags of a fragment are those of all its characters. `budget` is spent by the fragments returned; grouping stops once it is.
pub fn group_page_chars(
    chars: &[LayoutChar],
    width: u32,
    height: u32,
    budget: &mut LayoutBudget,
) -> Vec<LayoutLine> {
    let mut lines = Vec::new();
    let mut open: Option<Fragment> = None;
    for character in chars {
        if budget.stopped {
            break;
        }
        if is_text_whitespace(character.character) {
            if let Some(fragment) = open.as_mut() {
                fragment.pending_space = true;
            }
            continue;
        }
        let usable = !character.generated
            && !character.character.is_control()
            && character.font_size.is_finite()
            && character.font_size > 0.0
            && [
                character.left,
                character.right,
                character.bottom,
                character.top,
                character.baseline,
            ]
            .iter()
            .all(|value| value.is_finite());
        if !usable {
            continue;
        }
        let first = match &open {
            Some(fragment) if fragment.continues_with(character) => false,
            Some(_) => {
                if let Some(fragment) = open.take() {
                    lines.extend(finish(fragment, width, height, budget));
                }
                true
            }
            None => true,
        };
        if budget.stopped {
            break;
        }
        let fragment = open.get_or_insert_with(|| Fragment::start(character));
        if fragment.push(character, first) {
            budget.truncated = true;
        }
    }
    if let Some(fragment) = open.take().filter(|_| !budget.stopped) {
        lines.extend(finish(fragment, width, height, budget));
    }
    lines
}

/// Encodes a layout as the child's response body.
#[must_use]
pub fn encode_layout_response(layout: &PdfLayout) -> Vec<u8> {
    let pages: Vec<Value> = layout
        .pages
        .iter()
        .map(|page| {
            let lines: Vec<Value> = page
                .lines
                .iter()
                .map(|line| {
                    json!({
                        "text": line.text,
                        "x0": line.x0,
                        "y0": line.y0,
                        "x1": line.x1,
                        "y1": line.y1,
                        "size": line.font_size_centipoints,
                        "bold": line.bold,
                        "italic": line.italic,
                    })
                })
                .collect();
            json!({
                "page": page.page_number,
                "width": page.width_centipoints,
                "height": page.height_centipoints,
                "lines": lines,
            })
        })
        .collect();
    json!({"result": {
        "pageCount": layout.page_count,
        "pages": pages,
        "truncated": layout.truncated,
    }})
    .to_string()
    .into_bytes()
}

/// A non-negative integer member that fits `u32`. Floats and negatives are refused.
fn u32_member(object: &Map<String, Value>, key: &str) -> Result<u32, PdfError> {
    object
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or(PdfError::Layout)
}

/// A boolean member.
fn bool_member(object: &Map<String, Value>, key: &str) -> Result<bool, PdfError> {
    object
        .get(key)
        .and_then(Value::as_bool)
        .ok_or(PdfError::Layout)
}

/// Whether `text` is what the child's normalization leaves: non-empty, single spaces between
/// words, no other whitespace and no control characters.
fn is_normalized_line(text: &str) -> bool {
    let joined = text
        .split(is_text_whitespace)
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    !text.is_empty() && joined == text && !text.chars().any(char::is_control)
}

/// One line of a response, checked against its page and the per-line limits. `chars` is the
/// character count of the line.
fn decode_line(value: &Value, width: u32, height: u32) -> Result<(LayoutLine, usize), PdfError> {
    let object = value.as_object().ok_or(PdfError::Layout)?;
    let text = object
        .get("text")
        .and_then(Value::as_str)
        .filter(|text| is_normalized_line(text))
        .ok_or(PdfError::Layout)?;
    let chars = text.chars().count();
    if chars > MAX_LAYOUT_LINE_CHARS {
        return Err(PdfError::Layout);
    }
    let (x0, y0, x1, y1) = (
        u32_member(object, "x0")?,
        u32_member(object, "y0")?,
        u32_member(object, "x1")?,
        u32_member(object, "y1")?,
    );
    let size = u32_member(object, "size")?;
    let within = |value: u32, page: u32| value <= page.saturating_add(LAYOUT_BOUNDS_TOLERANCE);
    if x0 > x1
        || y0 > y1
        || !within(x1, width)
        || !within(y1, height)
        || !(1..=MAX_LAYOUT_FONT_SIZE).contains(&size)
    {
        return Err(PdfError::Layout);
    }
    Ok((
        LayoutLine {
            text: text.to_owned(),
            x0,
            y0,
            x1,
            y1,
            font_size_centipoints: size,
            bold: bool_member(object, "bold")?,
            italic: bool_member(object, "italic")?,
        },
        chars,
    ))
}

/// One page of a response. Returns the page and the characters and lines it holds.
fn decode_page(value: &Value, index: usize) -> Result<(PageLayout, usize, usize), PdfError> {
    let object = value.as_object().ok_or(PdfError::Layout)?;
    let page_number = u32_member(object, "page")?;
    if usize::try_from(page_number).ok() != Some(index + 1) {
        return Err(PdfError::Layout);
    }
    let width = u32_member(object, "width")?;
    let height = u32_member(object, "height")?;
    if width == 0 || height == 0 {
        return Err(PdfError::Layout);
    }
    let listed = object
        .get("lines")
        .and_then(Value::as_array)
        .filter(|listed| listed.len() <= MAX_LAYOUT_LINES)
        .ok_or(PdfError::Layout)?;
    let mut lines = Vec::with_capacity(listed.len());
    let mut chars = 0_usize;
    for line in listed {
        let (line, count) = decode_line(line, width, height)?;
        chars += count;
        lines.push(line);
    }
    Ok((
        PageLayout {
            page_number,
            width_centipoints: width,
            height_centipoints: height,
            lines,
        },
        chars,
        listed.len(),
    ))
}

/// Decodes and validates the child's layout response body. The child is untrusted: every field and
/// limit is checked here.
///
/// # Errors
///
/// Returns [`PdfError::PageLimit`] for the page-limit response, and [`PdfError::Layout`] for a
/// malformed body, a page list that disagrees with the page count, a page whose number is out of
/// order or whose size is zero, a non-integer, negative or out-of-range number, a line that is
/// empty, not normalized or over [`MAX_LAYOUT_LINE_CHARS`] characters, a box that is inverted or
/// lies outside its page by more than [`LAYOUT_BOUNDS_TOLERANCE`], a font size outside
/// `1..=`[`MAX_LAYOUT_FONT_SIZE`], or more than [`MAX_LAYOUT_LINES`] lines or
/// [`MAX_CHARACTERS_LIMIT`] characters in total.
pub fn decode_layout_response(body: &[u8]) -> Result<PdfLayout, PdfError> {
    let value: Value = serde_json::from_slice(body).map_err(|_| PdfError::Layout)?;
    if value.get("error").and_then(Value::as_str) == Some("page_limit") {
        return Err(PdfError::PageLimit);
    }
    let result = value
        .get("result")
        .and_then(Value::as_object)
        .ok_or(PdfError::Layout)?;
    let page_count = u32_member(result, "pageCount")
        .ok()
        .filter(|count| (1..=MAX_PAGE_COUNT).contains(count))
        .ok_or(PdfError::Layout)?;
    let listed = result
        .get("pages")
        .and_then(Value::as_array)
        .filter(|listed| u32::try_from(listed.len()).ok() == Some(page_count))
        .ok_or(PdfError::Layout)?;
    let mut pages = Vec::with_capacity(listed.len());
    let (mut total_chars, mut total_lines) = (0_usize, 0_usize);
    for (index, page) in listed.iter().enumerate() {
        let (page, chars, lines) = decode_page(page, index)?;
        total_chars += chars;
        total_lines += lines;
        if total_chars > MAX_CHARACTERS_LIMIT || total_lines > MAX_LAYOUT_LINES {
            return Err(PdfError::Layout);
        }
        pages.push(page);
    }
    Ok(PdfLayout {
        page_count,
        pages,
        truncated: bool_member(result, "truncated")?,
    })
}

/// Reads the layout of a PDF through the `pdf-layout` worker: positioned line fragments with font
/// information.
///
/// The content is checked against the import limit and the `%PDF-` header first. Any worker failure
/// is the fixed layout failure; a document over the page limit is [`PdfError::PageLimit`].
///
/// # Errors
///
/// Returns a [`PdfError`] for validation, page-limit, layout and cancellation outcomes.
pub async fn layout_pdf<C: Future<Output = ()>>(
    supervisor: &WorkerSupervisor,
    pdfium_dir: &Path,
    content: &[u8],
    cancel: C,
) -> Result<PdfLayout, PdfError> {
    validate_pdf_content(content, MAX_FILE_BYTES)?;
    let body = encode_request(PdfOperation::Layout, pdfium_dir, 0, content)
        .map_err(|_| PdfError::Layout)?;
    let response = supervisor
        .run(WorkerKind::PdfLayout, &body, cancel)
        .await
        .map_err(|error| map_worker_error(error, PdfOperation::Layout, PdfError::Layout))?;
    decode_layout_response(&response)
}
