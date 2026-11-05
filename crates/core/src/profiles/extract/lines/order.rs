//! Reading order of geometry lines, and the joining of wrapped continuation fragments.
//!
//! # Reading order
//!
//! Lines are grouped by page. Lines in the bottom 8 % of a page do not count for column detection. A page whose lines all have geometry is laid out by
//! [`order_page`]; a page with any line lacking geometry keeps its input order (column 0).
//!
//! 1. **Column detection.** Candidate gutters are the distinct `x0` values between 12 % and 85 % of
//!    the page width. For a candidate `g` (tolerance `t` = 1 % of the width), lines with `x0 >= g - t`
//!    are *right*, other lines with `x1 <= g + t` are *left*, the rest *span* the gutter. The candidate
//!    is a real column split when: at least three lines are left and three right; left plus right
//!    are at least 70 % of the lines (spanning lines are at most 30 %); at least 60 % of the lines
//!    of each side share one `x0` (columns are flush-left); fewer than half of the right side's
//!    `x1` values coincide (that is a right-aligned date or location, not a column); and the two sides
//!    overlap vertically. The candidate with the most lines on its smaller side wins, the lowest `g` on a tie. This
//!    covers a narrow Europass sidebar and two equal columns.
//!    A centered page header (a name above a sidebar) lies inside one side's span: a line that starts
//!    above the first line of the other column and is either not flush with the left edge of its own
//!    column or set at least 1.3 times larger than the column's median font is moved to the spanning
//!    lines.
//! 2. **Order.** With no split, lines go top to bottom (see rows below). With a split, spanning
//!    lines (a header across the page, a footer) cut the page into bands, and each band is read
//!    left column then right column, top to bottom, then the spanning line that closes the band.
//! 3. **Rows.** Within a column, lines whose tops differ by less than half the font size are one
//!    row and are read left to right (a title and its right-aligned date).
//!
//! Columns are reported as 0 (single column or spanning), 1 (left) and 2 (right).
//!
//! # Joining wrapped fragments
//!
//! After ordering, a line is appended to the previous one only when it is clearly the same field:
//!
//! * **URL tail.** The previous line ends with a token that starts with `http://`, `https://` or
//!   `www.` and has at least 20 characters; the tail is one token without whitespace that starts
//!   with a lowercase letter, digit or URL punctuation, and is not a heading, an email, a phone
//!   number, a year or a date, a URL start, or a domain (`github.com/x`). The fragments
//!   `https://example.com/alex-sa` and `mple` join without a space. Never across a blank line
//!   (`blank_before`).
//!   *With geometry* the wrap must be visible: same page and column, the previous fragment's right
//!   edge within two font sizes of the column's right margin (the longest line of the column), the
//!   tail starting within one font size of the column's left edge, and a vertical gap of at most one
//!   font size. *Without geometry* nothing shows a wrap, so the rule is stricter: the previous line
//!   is the URL alone (no whitespace), and the tail has at most 8 characters and no dot.
//! * **Address wrap.** The previous line ends with a comma and holds a digit or a street word, and
//!   the next line is short (at most 60 characters), holds a postal code or a state/province and no
//!   email, phone or URL, and does not follow a blank line. They join with a single space.
//!
//! With geometry the address rule also needs the same page and column, a left edge within one font
//! size, and a vertical gap of at most one font size.

use std::collections::{BTreeMap, HashMap};

use super::dates::{contains_date, contains_year};
use super::lexicon::{fold, has_street_word, heading_kind};
use super::locale::{find_phones, find_postal_codes, find_region};
use super::model::TextLine;
use crate::profiles::extract::pattern::{EMAIL, URL};

/// The share of the page width, in per cent, under which a candidate gutter is not considered.
const GUTTER_MIN_PERCENT: i32 = 12;
/// The share of the page width, in per cent, over which a candidate gutter is not considered.
const GUTTER_MAX_PERCENT: i32 = 85;

/// Lines in reading order with the column of each.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadingOrder {
    /// The lines in reading order, `index` renumbered from 0.
    pub lines: Vec<TextLine>,
    /// 0 single column or spanning, 1 left column, 2 right column; parallel to `lines`.
    pub columns: Vec<u8>,
}

/// Orders the lines of every page and joins wrapped fragments. See the module docs.
#[must_use]
pub fn read_lines(lines: Vec<TextLine>) -> ReadingOrder {
    let mut pages: BTreeMap<u32, Vec<TextLine>> = BTreeMap::new();
    for line in lines {
        pages.entry(line.page).or_default().push(line);
    }
    let mut ordered: Vec<(TextLine, u8)> = Vec::new();
    for (_, page_lines) in pages {
        ordered.extend(order_page(page_lines));
    }
    let joined = join_fragments(ordered);
    let mut result = ReadingOrder {
        lines: Vec::with_capacity(joined.len()),
        columns: Vec::with_capacity(joined.len()),
    };
    for (position, (mut line, column)) in joined.into_iter().enumerate() {
        line.index = u32::try_from(position).unwrap_or(u32::MAX);
        if let (Some(g), Some(previous)) = (line.geometry, result.lines.last())
            && let Some(p) = previous.geometry
            && previous.page == line.page
            && result.columns.last() == Some(&column)
        {
            let gap = i64::from(g.y0.saturating_sub(p.y1));
            line.blank_before = gap * 10 > i64::from(g.font_size_centipoints) * 4;
        }
        result.lines.push(line);
        result.columns.push(column);
    }
    result
}

/// The lines of one page in reading order, each with its column.
#[must_use]
pub fn order_page(lines: Vec<TextLine>) -> Vec<(TextLine, u8)> {
    if lines.iter().any(|line| line.geometry.is_none()) {
        return lines.into_iter().map(|line| (line, 0)).collect();
    }
    let Some(gutter) = detect_gutter(&lines) else {
        return row_order(lines).into_iter().map(|line| (line, 0)).collect();
    };
    let tolerance = tolerance(&lines);
    let mut left = Vec::new();
    let mut right = Vec::new();
    let mut spanning = Vec::new();
    for line in lines {
        match side(&line, gutter, tolerance) {
            1 => left.push(line),
            2 => right.push(line),
            _ => spanning.push(line),
        }
    }
    promote_headers(&mut left, &mut right, &mut spanning, tolerance);
    let spanning = row_order(spanning);
    let band_of = |line: &TextLine| {
        let top = geometry_top(line);
        spanning.iter().filter(|s| geometry_top(s) <= top).count()
    };
    let mut result = Vec::new();
    for band in 0..=spanning.len() {
        let in_band = |lines: &[TextLine]| -> Vec<TextLine> {
            lines
                .iter()
                .filter(|l| band_of(l) == band)
                .cloned()
                .collect()
        };
        result.extend(row_order(in_band(&left)).into_iter().map(|l| (l, 1)));
        result.extend(row_order(in_band(&right)).into_iter().map(|l| (l, 2)));
        if let Some(line) = spanning.get(band) {
            result.push((line.clone(), 0));
        }
    }
    result
}

/// Moves page headers that sit inside one column's span to the spanning lines: a line that starts
/// above the first line of the other column and is not flush with the left edge of its own column
/// (a centered name above a sidebar and a main column).
fn promote_headers(
    left: &mut Vec<TextLine>,
    right: &mut Vec<TextLine>,
    spanning: &mut Vec<TextLine>,
    tolerance: i32,
) {
    let median = |lines: &[TextLine]| {
        let mut values: Vec<i32> = lines
            .iter()
            .filter_map(|l| l.geometry)
            .map(|g| g.x0)
            .collect();
        values.sort_unstable();
        values.get(values.len() / 2).copied().unwrap_or(0)
    };
    let top = |lines: &[TextLine]| lines.iter().map(geometry_top).min().unwrap_or(i32::MAX);
    let (left_top, right_top) = (top(left), top(right));
    let median_font = |lines: &[TextLine]| {
        let mut values: Vec<i32> = lines
            .iter()
            .filter_map(|l| l.geometry)
            .map(|g| g.font_size_centipoints)
            .collect();
        values.sort_unstable();
        values.get(values.len() / 2).copied().unwrap_or(0)
    };
    let (left_x0, right_x0) = (median(left), median(right));
    let (left_font, right_font) = (median_font(left), median_font(right));
    let is_header = |line: &TextLine, edge: i32, font: i32| {
        line.geometry.is_some_and(|g| {
            g.x0.saturating_sub(edge).saturating_abs() > tolerance
                || i64::from(g.font_size_centipoints) * 10 >= i64::from(font) * 13
        })
    };
    let mut promote = |lines: &mut Vec<TextLine>, other_top: i32, edge: i32, font: i32| {
        let (header, rest): (Vec<TextLine>, Vec<TextLine>) = std::mem::take(lines)
            .into_iter()
            .partition(|line| geometry_top(line) < other_top && is_header(line, edge, font));
        *lines = rest;
        spanning.extend(header);
    };
    promote(right, left_top, right_x0, right_font);
    promote(left, right_top, left_x0, left_font);
}

fn geometry_top(line: &TextLine) -> i32 {
    line.geometry.map_or(0, |g| g.y0)
}

fn page_width(lines: &[TextLine]) -> i32 {
    lines
        .iter()
        .filter_map(|l| l.geometry)
        .map(|g| g.page_width_centipoints.max(g.x1))
        .max()
        .unwrap_or(0)
}

fn tolerance(lines: &[TextLine]) -> i32 {
    (page_width(lines) / 100).max(1)
}

/// 1 left, 2 right, 0 spanning, for a candidate gutter.
fn side(line: &TextLine, gutter: i32, tolerance: i32) -> u8 {
    match line.geometry {
        Some(g) if g.x0 >= gutter.saturating_sub(tolerance) => 2,
        Some(g) if g.x1 <= gutter.saturating_add(tolerance) => 1,
        _ => 0,
    }
}

/// Whether at least 60 % of the values lie within `tolerance` of their median.
fn clustered(values: &mut [i32], tolerance: i32) -> bool {
    if values.is_empty() {
        return false;
    }
    values.sort_unstable();
    let median = values[values.len() / 2];
    let near = values
        .iter()
        .filter(|v| v.saturating_sub(median).saturating_abs() <= tolerance)
        .count();
    near * 10 >= values.len() * 6
}

/// The `x0` of the gutter, or `None` for a single-column page.
fn detect_gutter(all_lines: &[TextLine]) -> Option<i32> {
    // Lines in the bottom 8 % of the page (page numbers, footers) say nothing about columns.
    let page_height = all_lines
        .iter()
        .filter_map(|l| l.geometry)
        .map(|g| i64::from(g.page_height_centipoints))
        .max()
        .unwrap_or(0);
    let lines: Vec<TextLine> = all_lines
        .iter()
        .filter(|l| {
            l.geometry
                .is_some_and(|g| i64::from(g.y0) * 100 <= page_height * 92)
        })
        .cloned()
        .collect();
    let lines = lines.as_slice();
    let width = page_width(lines);
    if width <= 0 || lines.len() < 6 {
        return None;
    }
    let tolerance = tolerance(lines);
    let mut candidates: Vec<i32> = lines
        .iter()
        .filter_map(|l| l.geometry)
        .map(|g| g.x0)
        .filter(|x| {
            i64::from(*x) * 100 > i64::from(width) * i64::from(GUTTER_MIN_PERCENT)
                && i64::from(*x) * 100 < i64::from(width) * i64::from(GUTTER_MAX_PERCENT)
        })
        .collect();
    candidates.sort_unstable();
    candidates.dedup();
    let mut best: Option<(usize, i32)> = None;
    for gutter in candidates {
        let (mut left, mut right) = (Vec::new(), Vec::new());
        let mut spanning = 0_usize;
        for line in lines {
            match side(line, gutter, tolerance) {
                1 => left.push(line),
                2 => right.push(line),
                _ => spanning += 1,
            }
        }
        let total = lines.len();
        if left.len() < 3
            || right.len() < 3
            || (left.len() + right.len()) * 10 < total * 7
            || spanning * 10 > total * 3
        {
            continue;
        }
        let mut left_x0: Vec<i32> = left
            .iter()
            .filter_map(|l| l.geometry)
            .map(|g| g.x0)
            .collect();
        let mut right_x0: Vec<i32> = right
            .iter()
            .filter_map(|l| l.geometry)
            .map(|g| g.x0)
            .collect();
        let mut right_x1: Vec<i32> = right
            .iter()
            .filter_map(|l| l.geometry)
            .map(|g| g.x1)
            .collect();
        if !clustered(&mut left_x0, tolerance)
            || !clustered(&mut right_x0, tolerance)
            || count_near_median(&mut right_x1, tolerance) * 2 >= right_x1.len()
        {
            continue;
        }
        let span = |lines: &[&TextLine]| -> (i32, i32) {
            let tops = lines.iter().filter_map(|l| l.geometry).map(|g| g.y0);
            let bottoms = lines.iter().filter_map(|l| l.geometry).map(|g| g.y1);
            (tops.min().unwrap_or(0), bottoms.max().unwrap_or(0))
        };
        let (left_top, left_bottom) = span(&left);
        let (right_top, right_bottom) = span(&right);
        if left_bottom <= right_top || right_bottom <= left_top {
            continue;
        }
        let score = left.len().min(right.len());
        if best.is_none_or(|(best_score, _)| score > best_score) {
            best = Some((score, gutter));
        }
    }
    best.map(|(_, gutter)| gutter)
}

fn count_near_median(values: &mut [i32], tolerance: i32) -> usize {
    values.sort_unstable();
    let median = values[values.len() / 2];
    values
        .iter()
        .filter(|v| v.saturating_sub(median).saturating_abs() <= tolerance)
        .count()
}

/// Top-to-bottom order; lines of one row (tops within half a font size) go left to right.
fn row_order(mut lines: Vec<TextLine>) -> Vec<TextLine> {
    lines.sort_by_key(geometry_top);
    let mut rows: Vec<Vec<TextLine>> = Vec::new();
    let mut row_top = i32::MIN;
    for line in lines {
        let top = geometry_top(&line);
        let half_font = line.geometry.map_or(0, |g| g.font_size_centipoints / 2);
        match rows.last_mut() {
            Some(row) if row_top != i32::MIN && top.saturating_sub(row_top) <= half_font => {
                row.push(line)
            }
            _ => {
                row_top = top;
                rows.push(vec![line]);
            }
        }
    }
    rows.into_iter()
        .flat_map(|mut row| {
            row.sort_by_key(|l| l.geometry.map_or(0, |g| g.x0));
            row
        })
        .collect()
}

/// The last whitespace-delimited token of a text.
fn last_token(text: &str) -> &str {
    text.split_whitespace().next_back().unwrap_or("")
}

fn is_url_start(token: &str) -> bool {
    let lower = fold(token);
    lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("www.")
}

/// Whether the text is a domain (`github.com/example`), not the tail of a wrapped URL.
fn looks_like_domain(text: &str) -> bool {
    text.split_once('.').is_some_and(|(host, rest)| {
        !host.is_empty()
            && host.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
            && rest.chars().take_while(char::is_ascii_alphabetic).count() >= 2
            && rest
                .trim_start_matches(|c: char| c.is_ascii_alphabetic())
                .chars()
                .next()
                .is_none_or(|c| c == '/' || c == '.')
    })
}

/// Whether the text can be the tail of a wrapped URL (see the module docs).
fn is_url_tail(text: &str, allow_dot: bool, max_chars: usize) -> bool {
    let count = text.chars().count();
    (1..=max_chars).contains(&count)
        && !text.contains(char::is_whitespace)
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._~/%#?=&+:-".contains(c))
        && text.chars().next().is_some_and(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || "._~/%#?=&-".contains(c)
        })
        && (allow_dot || !text.contains('.'))
        && !is_url_start(text)
        && !looks_like_domain(text)
        && !EMAIL.is_match(text)
        && find_phones(text).is_empty()
        && !contains_year(text)
        && !contains_date(text)
        && heading_kind(text).is_none()
}

/// The left and right edge of each page column, from the longest and the leftmost lines.
type ColumnEdges = HashMap<(u32, u8), (i32, i32)>;

/// Whether the geometry (when both lines have it) says `next` continues `previous`.
fn adjacent(previous: &TextLine, next: &TextLine, previous_column: u8, next_column: u8) -> bool {
    match (previous.geometry, next.geometry) {
        (Some(a), Some(b)) => {
            let font = a.font_size_centipoints.max(1);
            previous.page == next.page
                && previous_column == next_column
                && a.x0.saturating_sub(b.x0).saturating_abs() <= font
                && b.y0.saturating_sub(a.y1) <= font
        }
        _ => true,
    }
}

fn continues_url(
    previous: &TextLine,
    next: &TextLine,
    column: u8,
    next_column: u8,
    edges: &ColumnEdges,
) -> bool {
    if next.blank_before
        || previous.text.chars().count() < 20
        || !is_url_start(last_token(&previous.text))
    {
        return false;
    }
    match (previous.geometry, next.geometry) {
        (Some(a), Some(b)) => {
            let font = a.font_size_centipoints.max(1);
            let Some((left, right)) = edges.get(&(previous.page, column)).copied() else {
                return false;
            };
            is_url_tail(&next.text, true, 24)
                && adjacent(previous, next, column, next_column)
                && right.saturating_sub(a.x1) <= font.saturating_mul(2)
                && b.x0.saturating_sub(left).saturating_abs() <= font
        }
        _ => !previous.text.contains(char::is_whitespace) && is_url_tail(&next.text, false, 8),
    }
}

fn continues_address(previous: &TextLine, next: &TextLine) -> bool {
    let text = next.text.as_str();
    !next.blank_before
        && previous.text.ends_with(',')
        && (previous.text.chars().any(|c| c.is_ascii_digit()) || has_street_word(&previous.text))
        && text.chars().count() <= 60
        && (!find_postal_codes(text).is_empty() || find_region(text).is_some())
        && !EMAIL.is_match(text)
        && !URL.is_match(text)
        && find_phones(text).is_empty()
}

/// Joins wrapped fragments (see the module docs).
fn join_fragments(items: Vec<(TextLine, u8)>) -> Vec<(TextLine, u8)> {
    let mut edges: ColumnEdges = HashMap::new();
    for (line, column) in &items {
        if let Some(g) = line.geometry {
            let edge = edges.entry((line.page, *column)).or_insert((g.x0, g.x1));
            edge.0 = edge.0.min(g.x0);
            edge.1 = edge.1.max(g.x1);
        }
    }
    let mut result: Vec<(TextLine, u8)> = Vec::with_capacity(items.len());
    for (line, column) in items {
        if let Some((previous, previous_column)) = result.last_mut() {
            let separator = if continues_url(previous, &line, *previous_column, column, &edges) {
                Some("")
            } else if continues_address(previous, &line) {
                Some(" ")
            } else {
                None
            };
            if let Some(separator) = separator
                && adjacent(previous, &line, *previous_column, column)
            {
                previous.text.push_str(separator);
                previous.text.push_str(&line.text);
                if let (Some(a), Some(b)) = (previous.geometry.as_mut(), line.geometry) {
                    a.x0 = a.x0.min(b.x0);
                    a.y0 = a.y0.min(b.y0);
                    a.x1 = a.x1.max(b.x1);
                    a.y1 = a.y1.max(b.y1);
                }
                continue;
            }
        }
        result.push((line, column));
    }
    result
}
