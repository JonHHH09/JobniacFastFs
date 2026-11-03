//! Conversion of the escaped HTML fragments of Europass rich-text fields to plain-text blocks.
//!
//! The editor writes `<p>`, `<br>`, `<ul><li>` and inline tags (`<strong>`, `<em>`, ...). Block tags
//! split blocks, a paragraph holding only `<br>` is a [`RichBlock::Blank`], list items become
//! [`RichBlock::Bullet`], every other tag is dropped and its text kept. Character references are
//! decoded once with `jobniac_text::html::html4_unescape`; decoded text is never re-read as markup.

use jobniac_text::html::html4_unescape;

use super::model::RichBlock;

/// Tags that start and end a block.
fn is_block(name: &str) -> bool {
    matches!(
        name,
        "p" | "div"
            | "li"
            | "ul"
            | "ol"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "blockquote"
            | "table"
            | "thead"
            | "tbody"
            | "tr"
            | "td"
            | "th"
    )
}

#[derive(Default)]
struct Blocks {
    out: Vec<RichBlock>,
    line: String,
    saw_break: bool,
    in_item: bool,
}

impl Blocks {
    fn text(&mut self, raw: &str) {
        let mut previous_space = self.line.ends_with(' ') || self.line.ends_with('\n');
        for character in raw.chars() {
            if character.is_ascii_whitespace() {
                if !previous_space {
                    self.line.push(' ');
                }
                previous_space = true;
            } else {
                self.line.push(character);
                previous_space = false;
            }
        }
    }

    fn line_break(&mut self) {
        if self.line.trim().is_empty() {
            self.line.clear();
            self.saw_break = true;
        } else {
            self.line.push('\n');
        }
    }

    fn flush(&mut self) {
        let decoded = html4_unescape(Some(&self.line));
        let cleaned: String = decoded
            .chars()
            .map(|c| match c {
                '\u{A0}' => ' ',
                '\n' => '\n',
                c if c.is_control() => ' ',
                c => c,
            })
            .collect();
        let lines: Vec<&str> = cleaned.split('\n').map(str::trim).collect();
        let text = lines.join("\n").trim_matches('\n').to_owned();
        if !text.is_empty() {
            self.out.push(if self.in_item {
                RichBlock::Bullet(text)
            } else {
                RichBlock::Paragraph(text)
            });
        } else if self.saw_break {
            self.out.push(RichBlock::Blank);
        }
        self.line.clear();
        self.saw_break = false;
    }
}

/// Convert a markup fragment to blocks.
pub(super) fn blocks_from_markup(markup: &str) -> Vec<RichBlock> {
    let mut blocks = Blocks::default();
    let mut rest = markup;
    while let Some(open) = rest.find('<') {
        blocks.text(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find('>') else {
            // An unterminated tag is literal text.
            blocks.text(&rest[open..]);
            rest = "";
            break;
        };
        let tag = &after[..close];
        rest = &after[close + 1..];
        let closing = tag.starts_with('/');
        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(char::is_ascii_alphanumeric)
            .collect::<String>()
            .to_ascii_lowercase();
        if name == "br" {
            blocks.line_break();
        } else if is_block(&name) {
            blocks.flush();
            if name == "li" {
                blocks.in_item = !closing;
            }
        }
    }
    blocks.text(rest);
    blocks.flush();
    blocks.out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(text: &str) -> RichBlock {
        RichBlock::Paragraph(text.to_owned())
    }

    #[test]
    fn paragraphs_and_blank_separators() {
        assert_eq!(
            blocks_from_markup("<p>One</p><p><br></p><p>Two &amp; three</p>"),
            vec![p("One"), RichBlock::Blank, p("Two & three")]
        );
    }

    #[test]
    fn lists_and_inline_tags() {
        assert_eq!(
            blocks_from_markup(
                "<p>Intro <strong>bold</strong></p><ul><li>a</li><li><em>b</em></li></ul><p>End</p>"
            ),
            vec![
                p("Intro bold"),
                RichBlock::Bullet("a".into()),
                RichBlock::Bullet("b".into()),
                p("End")
            ]
        );
    }

    #[test]
    fn soft_breaks_and_bare_text() {
        assert_eq!(blocks_from_markup("line<br>two"), vec![p("line\ntwo")]);
        assert_eq!(blocks_from_markup("  just   text "), vec![p("just text")]);
        assert!(blocks_from_markup("").is_empty());
    }

    #[test]
    fn table_cells_and_rows_are_separators() {
        assert_eq!(
            blocks_from_markup("<table><tr><td>a</td><td>b</td></tr><tr><td>c</td></tr></table>"),
            vec![p("a"), p("b"), p("c")]
        );
    }

    #[test]
    fn decoded_entities_are_not_markup() {
        assert_eq!(
            blocks_from_markup("<p>1 &lt;b&gt; 2</p>"),
            vec![p("1 <b> 2")]
        );
        assert_eq!(blocks_from_markup("<p>broken <b"), vec![p("broken <b")]);
    }
}
