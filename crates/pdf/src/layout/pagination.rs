//! Forward-only pagination of prepared rows and its keep rules.

use super::text::{
    is_bullet_continuation_line, is_bullet_line, is_labeled_line, is_resume_date_line,
    is_section_heading,
};
use super::{BODY_END_Y, BODY_START_Y, Leading};

/// One page of rows and the index of its first row within the whole layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageLines {
    /// The rows drawn on this page.
    pub lines: Vec<String>,
    /// The index in the whole row list of the first row of this page.
    pub start_index: usize,
}

/// Rows that fit on one page at this leading.
pub fn page_capacity(leading: Leading) -> usize {
    let span = BODY_START_Y - BODY_END_Y;
    let rows = (span / leading.value()).floor();
    // `rows` is a small non-negative integer-valued float, so the cast is exact.
    (rows as usize + 1).max(1)
}

fn is_resume_entry_heading(lines: &[String], index: usize) -> bool {
    if index + 1 >= lines.len() {
        return false;
    }
    let line = &lines[index];
    line.contains(" | ")
        && !is_labeled_line(line)
        && !is_section_heading(line)
        && is_resume_date_line(&lines[index + 1])
}

fn wrapped_bullet_count(lines: &[String], index: usize) -> usize {
    let mut end = index + 1;
    while end < lines.len() && is_bullet_continuation_line(&lines[end]) {
        end += 1;
    }
    end - index
}

fn entry_opening_count(lines: &[String], index: usize) -> usize {
    let first_detail = index + 2;
    if first_detail < lines.len() && is_bullet_line(&lines[first_detail]) {
        return 2 + wrapped_bullet_count(lines, first_detail);
    }
    // The source requires three slots even when the third detail is absent.
    3
}

fn should_start_page(lines: &[String], index: usize, page_size: usize, capacity: usize) -> bool {
    if page_size == 0 {
        return false;
    }
    let remaining = capacity - page_size;
    let line = &lines[index];
    (is_section_heading(line) && remaining < 2)
        || (is_resume_entry_heading(lines, index) && remaining < entry_opening_count(lines, index))
        || (is_bullet_line(line) && remaining < wrapped_bullet_count(lines, index))
}

/// Splits rows into pages.
///
/// Oversized blocks may split, and the walk never produces an empty page except for an empty
/// row list.
pub fn paginate_lines(lines: &[String], leading: Leading) -> Vec<PageLines> {
    let capacity = page_capacity(leading);
    let mut pages = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut start = 0;
    for index in 0..lines.len() {
        if should_start_page(lines, index, current.len(), capacity) {
            pages.push(PageLines {
                lines: std::mem::take(&mut current),
                start_index: start,
            });
            start = index;
        }
        current.push(lines[index].clone());
        if current.len() == capacity {
            pages.push(PageLines {
                lines: std::mem::take(&mut current),
                start_index: start,
            });
            start = index + 1;
        }
    }
    if !current.is_empty() || pages.is_empty() {
        pages.push(PageLines {
            lines: current,
            start_index: start,
        });
    }
    pages
}
