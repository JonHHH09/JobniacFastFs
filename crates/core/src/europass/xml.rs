//! Shared reading helpers: namespace-aware child lookup, bounded text, dates and recorded problems.

use jobniac_text::java::truncate_utf16;
use roxmltree::Node;

use super::model::{PartialDate, RichBlock, RichText};
use super::rich_text::blocks_from_markup;

/// Longest short text field (names, titles, skill names), in UTF-16 units. Same budget as the
/// description limit of resume extraction.
pub const SHORT_TEXT_LIMIT: usize = 500;

/// Longest rich-text field (summary, job description, letter body), counted over the plain text of
/// its blocks in UTF-16 units.
pub const RICH_TEXT_LIMIT: usize = 10_000;

/// Most blocks a rich-text field keeps.
pub const MAX_BLOCKS: usize = 500;

/// Most entries any list keeps in total (employment, education, languages, skills, contact
/// channels, address lines, recorded problems, ...).
pub const MAX_ENTRIES: usize = 200;

pub(super) type Elem<'a, 'i> = Node<'a, 'i>;

fn is_named(node: &Node<'_, '_>, ns: &str, name: &str) -> bool {
    node.is_element() && node.tag_name().name() == name && node.tag_name().namespace() == Some(ns)
}

/// The first child element with this namespace and local name.
pub(super) fn child<'a, 'i>(node: Elem<'a, 'i>, ns: &str, name: &str) -> Option<Elem<'a, 'i>> {
    node.children().find(|c| is_named(c, ns, name))
}

/// All child elements with this namespace and local name. The node limit bounds the count; callers
/// cap what they keep through [`Reader::has_room`] or [`Reader::push`].
pub(super) fn children<'a, 'i: 'a>(
    node: Elem<'a, 'i>,
    ns: &'a str,
    name: &'a str,
) -> impl Iterator<Item = Elem<'a, 'i>> + 'a {
    node.children().filter(move |c| is_named(c, ns, name))
}

/// The concatenated direct text of an element, untrimmed.
fn raw_text(node: Elem<'_, '_>) -> String {
    node.children()
        .filter(|c| c.is_text())
        .filter_map(|c| c.text())
        .collect()
}

/// Control characters become spaces.
fn clean(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// An element that carries data: element children, non-blank text or any attribute other than
/// namespace declarations.
pub(super) fn has_content(node: Elem<'_, '_>) -> bool {
    node.children().any(|c| c.is_element())
        || !raw_text(node).trim().is_empty()
        || node.attributes().next().is_some()
}

/// `parent[index].field`, the path form of every recorded problem inside a list.
pub(super) fn at(list: &str, index: usize, field: &str) -> String {
    format!("{list}[{index}].{field}")
}

/// Collects what the parser cut, dropped or could not read, and turns element text into bounded
/// values.
#[derive(Debug, Default)]
pub(super) struct Reader {
    /// Paths of fields or lists cut at a cap.
    pub(super) truncated: Vec<String>,
    /// Paths of values that were malformed and dropped.
    pub(super) invalid: Vec<String>,
    /// Names of non-empty elements the parser does not map.
    pub(super) unsupported: Vec<String>,
}

impl Reader {
    fn record(list: &mut Vec<String>, overflow: &mut Vec<String>, path: &str, flag: &str) {
        if list.iter().any(|existing| existing == path) {
            return;
        }
        if list.len() >= MAX_ENTRIES {
            if !overflow.iter().any(|existing| existing == flag) && overflow.len() < MAX_ENTRIES {
                overflow.push(flag.to_owned());
            }
            return;
        }
        list.push(path.to_owned());
    }

    /// Record a field or list cut at a cap (once per path).
    pub(super) fn note_truncated(&mut self, path: &str) {
        if self.truncated.len() < MAX_ENTRIES && !self.truncated.iter().any(|e| e == path) {
            self.truncated.push(path.to_owned());
        }
    }

    /// Record a malformed value that was dropped.
    pub(super) fn note_invalid(&mut self, path: &str) {
        Self::record(
            &mut self.invalid,
            &mut self.truncated,
            path,
            "invalidFields",
        );
    }

    /// Record a non-empty element that is not mapped.
    pub(super) fn note_unsupported(&mut self, name: &str) {
        Self::record(
            &mut self.unsupported,
            &mut self.truncated,
            name,
            "unsupportedSections",
        );
    }

    /// Whether a list of `len` entries may take one more; records the list path when it may not.
    pub(super) fn has_room(&mut self, len: usize, path: &str) -> bool {
        if len < MAX_ENTRIES {
            true
        } else {
            self.note_truncated(path);
            false
        }
    }

    /// Push into a capped list, recording the list path when the cap drops the item.
    pub(super) fn push<T>(&mut self, list: &mut Vec<T>, item: T, path: &str) {
        if self.has_room(list.len(), path) {
            list.push(item);
        }
    }

    fn bounded(&mut self, text: &str, limit: usize, path: &str) -> String {
        let cut = truncate_utf16(text, limit);
        if cut.len() != text.len() {
            self.note_truncated(path);
        }
        cut.to_owned()
    }

    /// Trimmed text of a short field; blank is `None`. Control characters become spaces.
    pub(super) fn short(&mut self, node: Option<Elem<'_, '_>>, path: &str) -> Option<String> {
        let cleaned = clean(&raw_text(node?));
        let trimmed = cleaned.trim();
        if trimmed.is_empty() {
            return None;
        }
        Some(
            self.bounded(trimmed, SHORT_TEXT_LIMIT, path)
                .trim_end()
                .to_owned(),
        )
    }

    /// A short attribute value, cleaned like element text.
    pub(super) fn attr(
        &mut self,
        node: Option<Elem<'_, '_>>,
        name: &str,
        path: &str,
    ) -> Option<String> {
        let cleaned = clean(node?.attribute(name)?);
        let trimmed = cleaned.trim();
        if trimmed.is_empty() {
            return None;
        }
        Some(
            self.bounded(trimmed, SHORT_TEXT_LIMIT, path)
                .trim_end()
                .to_owned(),
        )
    }

    /// An escaped-HTML field as blocks plus the original markup; blank is `None`.
    ///
    /// Blocks are built from the whole text (the input size is already capped), then capped by
    /// [`RICH_TEXT_LIMIT`] units and [`MAX_BLOCKS`], so a cut never lands inside a tag or entity.
    /// The markup is kept only when nothing was cut. Real child elements (unescaped HTML) are not
    /// read; they are reported in the unsupported sections.
    pub(super) fn rich(&mut self, node: Option<Elem<'_, '_>>, path: &str) -> Option<RichText> {
        let node = node?;
        for element in node.children().filter(|c| c.is_element()) {
            self.note_unsupported(&format!(
                "{}/{}",
                node.tag_name().name(),
                element.tag_name().name()
            ));
        }
        let raw = raw_text(node);
        let markup = raw.trim();
        if markup.is_empty() {
            return None;
        }
        let mut blocks = blocks_from_markup(markup);
        let mut cut = false;
        if blocks.len() > MAX_BLOCKS {
            blocks.truncate(MAX_BLOCKS);
            cut = true;
        }
        let mut budget = RICH_TEXT_LIMIT;
        let mut kept = Vec::with_capacity(blocks.len());
        for block in blocks {
            let (text, make): (String, fn(String) -> RichBlock) = match block {
                RichBlock::Paragraph(text) => (text, RichBlock::Paragraph),
                RichBlock::Bullet(text) => (text, RichBlock::Bullet),
                RichBlock::Blank => {
                    kept.push(RichBlock::Blank);
                    continue;
                }
            };
            let units = text.encode_utf16().count();
            if units <= budget {
                budget -= units;
                kept.push(make(text));
            } else {
                if budget > 0 {
                    kept.push(make(truncate_utf16(&text, budget).trim_end().to_owned()));
                }
                cut = true;
                break;
            }
        }
        if cut || markup.encode_utf16().count() > RICH_TEXT_LIMIT {
            self.note_truncated(path);
            return Some(RichText {
                markup: None,
                blocks: kept,
            });
        }
        Some(RichText {
            markup: Some(markup.to_owned()),
            blocks: kept,
        })
    }

    /// An ISO partial date; blank is `None`, malformed is dropped and recorded.
    pub(super) fn date(&mut self, node: Option<Elem<'_, '_>>, path: &str) -> Option<PartialDate> {
        let text = self.short(node, path)?;
        let date = PartialDate::parse(&text);
        if date.is_none() {
            self.note_invalid(path);
        }
        date
    }

    /// An `xs:boolean`; blank is `None`, anything but `true`/`false`/`1`/`0` is dropped and recorded.
    pub(super) fn boolean(&mut self, node: Option<Elem<'_, '_>>, path: &str) -> Option<bool> {
        let value = match self.short(node, path)?.as_str() {
            "true" | "1" => Some(true),
            "false" | "0" => Some(false),
            _ => None,
        };
        if value.is_none() {
            self.note_invalid(path);
        }
        value
    }
}
