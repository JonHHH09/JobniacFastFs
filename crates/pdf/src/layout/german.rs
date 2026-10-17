//! German A4 layout: validated resume and cover-letter content to fixed rows and pages.
//!
//! Every coordinate, height, indent, font size and width is IEEE-754 binary32, computed in the
//! operation order of the layout (`f32` after each addition, division and multiplication).
//! Glyph widths come from [`crate::metrics`]. The layout is pure: the caller runs the content
//! budgets (`budget_resume_content` and `budget_cover_letter_content`) before it calls
//! [`build_resume_layout`] or [`build_cover_letter_layout`].

use jobniac_text::java::java_strip;

use crate::error::PdfError;
use crate::metrics::{encode_german_font_text, german_font_text_width};

/// Page width in points (binary32-exact A4 width).
pub const PAGE_WIDTH: f64 = 595.275634765625;
/// Page height in points (binary32-exact A4 height).
pub const PAGE_HEIGHT: f64 = 841.8897705078125;
/// Left and right margin in points.
pub const MARGIN_X: f64 = 42.0;
/// Baseline of the first row of a page.
pub const TOP_Y: f32 = (PAGE_HEIGHT - 44.0) as f32;
/// Lowest baseline of a row.
pub const BOTTOM_Y: f32 = 38.0;
/// Width available to a row, in points.
pub const CONTENT_WIDTH: f32 = (PAGE_WIDTH - 84.0) as f32;
/// Height available to blocks on one page.
pub const PAGE_CAPACITY: f32 = TOP_Y - BOTTOM_Y;
/// A resume's last page must hold at least this much height (40 per cent of the capacity).
pub const MIN_LAST_PAGE_USAGE: f32 = PAGE_CAPACITY * 0.4;

/// A colour as 8-bit RGB.
pub type Rgb = [u8; 3];

const TEXT: Rgb = [28, 32, 36];
const MUTED: Rgb = [91, 99, 107];
const ACCENT: Rgb = [55, 71, 79];

/// Which layout a block belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Document {
    /// The German resume, one or two A4 pages.
    Resume,
    /// The German cover letter, one or two A4 pages.
    CoverLetter,
}

/// The role of a style. The names are the upper-case member names used in the golden vectors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Name heading.
    Name,
    /// Contact line.
    Contact,
    /// Horizontal rule row.
    Divider,
    /// Section heading with a rule below it (resume only).
    Section,
    /// Entry heading (resume only).
    Entry,
    /// Date and location line.
    Meta,
    /// Body text.
    Body,
    /// First row of a bullet (resume only).
    Bullet,
    /// Project link (resume only).
    Link,
    /// Blank vertical space.
    Spacer,
    /// Recipient line (cover letter only).
    Recipient,
    /// Subject line (cover letter only).
    Subject,
    /// Signature line (cover letter only).
    Signature,
}

impl Kind {
    /// Every kind, in declaration order.
    pub const ALL: [Kind; 13] = [
        Kind::Name,
        Kind::Contact,
        Kind::Divider,
        Kind::Section,
        Kind::Entry,
        Kind::Meta,
        Kind::Body,
        Kind::Bullet,
        Kind::Link,
        Kind::Spacer,
        Kind::Recipient,
        Kind::Subject,
        Kind::Signature,
    ];

    /// The upper-case member name (for example `BODY`).
    pub fn name(self) -> &'static str {
        match self {
            Self::Name => "NAME",
            Self::Contact => "CONTACT",
            Self::Divider => "DIVIDER",
            Self::Section => "SECTION",
            Self::Entry => "ENTRY",
            Self::Meta => "META",
            Self::Body => "BODY",
            Self::Bullet => "BULLET",
            Self::Link => "LINK",
            Self::Spacer => "SPACER",
            Self::Recipient => "RECIPIENT",
            Self::Subject => "SUBJECT",
            Self::Signature => "SIGNATURE",
        }
    }
}

/// Font size, leading, weight and colour of one kind of row.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style {
    /// The role of this style.
    pub kind: Kind,
    /// Font size in points, binary32.
    pub font_size: f32,
    /// Row height in points, binary32.
    pub leading: f32,
    /// Whether the row uses Helvetica-Bold.
    pub bold: bool,
    /// Text colour.
    pub color: Rgb,
}

const fn style(kind: Kind, font_size: f32, leading: f32, bold: bool, color: Rgb) -> Style {
    Style {
        kind,
        font_size,
        leading,
        bold,
        color,
    }
}

/// The style of a kind in a document, or `None` when the document has no such kind.
pub fn style_of(document: Document, kind: Kind) -> Option<Style> {
    match document {
        Document::Resume => resume_style(kind),
        Document::CoverLetter => cover_letter_style(kind),
    }
}

fn resume_style(kind: Kind) -> Option<Style> {
    use Kind::*;
    Some(match kind {
        Name => style(Name, 22.0, 25.0, true, TEXT),
        Contact => style(Contact, 8.5, 10.0, false, MUTED),
        Divider => style(Divider, 0.0, 12.0, false, ACCENT),
        Section => style(Section, 10.0, 15.0, true, ACCENT),
        Entry => style(Entry, 9.5, 11.0, true, TEXT),
        Meta => style(Meta, 8.5, 10.0, false, MUTED),
        Body => style(Body, 8.7, 10.2, false, TEXT),
        Bullet => style(Bullet, 8.7, 10.2, false, TEXT),
        Link => style(Link, 8.2, 9.5, false, MUTED),
        Spacer => style(Spacer, 0.0, 0.0, false, TEXT),
        Recipient | Subject | Signature => return None,
    })
}

fn cover_letter_style(kind: Kind) -> Option<Style> {
    use Kind::*;
    Some(match kind {
        Name => style(Name, 20.0, 23.0, true, TEXT),
        Contact => style(Contact, 8.5, 10.0, false, MUTED),
        Recipient => style(Recipient, 9.5, 12.0, true, TEXT),
        Meta => style(Meta, 8.5, 10.0, false, MUTED),
        Subject => style(Subject, 12.0, 16.0, true, ACCENT),
        Body => style(Body, 10.0, 14.0, false, TEXT),
        Signature => style(Signature, 10.0, 14.0, true, TEXT),
        Divider => style(Divider, 0.0, 12.0, false, ACCENT),
        Spacer => style(Spacer, 0.0, 0.0, false, TEXT),
        Section | Entry | Bullet | Link => return None,
    })
}

/// One physical row: sanitized text and its style, indent, height and font size.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderLine {
    /// The sanitized text (empty for spacers and dividers).
    pub text: String,
    /// The style of the row.
    pub style: Style,
    /// Horizontal indent in points, binary32.
    pub indent: f32,
    /// Vertical advance of the row in points, binary32.
    pub height: f32,
    /// Font size of the row in points, binary32.
    pub font_size: f32,
}

/// An atomic group of rows that is never split across pages.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    /// The rows of the block, in order.
    pub lines: Vec<RenderLine>,
}

impl Block {
    /// A block of rows.
    pub fn new(lines: Vec<RenderLine>) -> Self {
        Self { lines }
    }

    /// The height of the block: the exact sum of the row heights, rounded to
    /// binary32.
    pub fn height(&self) -> f32 {
        fsum(self.lines.iter().map(|line| f64::from(line.height))) as f32
    }
}

/// One laid-out page: its blocks and the height they use.
#[derive(Debug, Clone, PartialEq)]
pub struct PageLayout {
    /// The blocks on the page, in order.
    pub blocks: Vec<Block>,
    /// The height used by the blocks, binary32.
    pub used_height: f32,
}

/// A complete German layout: one or two pages and whether the footer carries the `Seite` label.
#[derive(Debug, Clone, PartialEq)]
pub struct GermanLayout {
    /// The pages in order.
    pub pages: Vec<PageLayout>,
    /// True for the German resume and every cover letter; false for an English resume.
    pub german_footer: bool,
}

/// A personal contact field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonalField {
    /// The field label.
    pub label: String,
    /// The field value.
    pub value: String,
}

/// A dated experience.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExperienceEntry {
    /// Job title.
    pub title: String,
    /// Employer.
    pub company: String,
    /// Location, when given.
    pub location: Option<String>,
    /// ISO start date, when given.
    pub start_date: Option<String>,
    /// ISO end date, when given.
    pub end_date: Option<String>,
    /// Achievement bullets.
    pub bullets: Vec<String>,
}

/// A dated education entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EducationEntry {
    /// Degree.
    pub degree: String,
    /// Institution.
    pub institution: String,
    /// Location, when given.
    pub location: Option<String>,
    /// ISO start date, when given.
    pub start_date: Option<String>,
    /// ISO end date, when given.
    pub end_date: Option<String>,
    /// Bullets.
    pub bullets: Vec<String>,
}

/// A group of skills.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillGroup {
    /// Category label.
    pub category: String,
    /// Skills in order.
    pub skills: Vec<String>,
}

/// A spoken language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageEntry {
    /// Language name.
    pub language: String,
    /// Proficiency, when given.
    pub proficiency: Option<String>,
}

/// A project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdditionalEntry {
    /// Project title.
    pub title: String,
    /// Project link, when given.
    pub organization: Option<String>,
    /// Bullets.
    pub bullets: Vec<String>,
}

/// Validated resume content.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StructuredResumeContent {
    /// Full name.
    pub full_name: String,
    /// Language code; `de` selects the German headings and footer.
    pub language: String,
    /// Optional profile summary.
    pub summary: Option<String>,
    /// Contact fields in order.
    pub personal_fields: Vec<PersonalField>,
    /// Professional experiences in order.
    pub experiences: Vec<ExperienceEntry>,
    /// Projects in order.
    pub additional: Vec<AdditionalEntry>,
    /// Education in order.
    pub education: Vec<EducationEntry>,
    /// Skill groups in order.
    pub skill_groups: Vec<SkillGroup>,
    /// Languages in order.
    pub languages: Vec<LanguageEntry>,
}

/// Validated German cover-letter content.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GermanCoverLetterContent {
    /// Sender name.
    pub sender_name: String,
    /// Contact fields in order.
    pub personal_fields: Vec<PersonalField>,
    /// Recipient company, when given.
    pub recipient_company: Option<String>,
    /// Recipient location, when given.
    pub recipient_location: Option<String>,
    /// Subject line.
    pub subject: String,
    /// Salutation.
    pub salutation: String,
    /// Body paragraphs in order.
    pub paragraphs: Vec<String>,
    /// Closing line.
    pub closing: String,
    /// Signature line.
    pub signature: String,
}

/// Builds the German resume layout.
///
/// # Errors
///
/// Returns a validation error for content that does not fit one or two resume pages, and
/// [`PdfError::Internal`] for an impossible measurement.
pub fn build_resume_layout(content: &StructuredResumeContent) -> Result<GermanLayout, PdfError> {
    let builder = Builder::new(Document::Resume);
    let german = content.language == "de";
    let mut blocks = vec![builder.header(&content.full_name, &content.personal_fields)?];
    if let Some(summary) = &content.summary {
        let mut lines = builder.section_prefix(0, if german { "PROFIL" } else { "PROFILE" })?;
        lines.extend(builder.wrap(summary, builder.style(Kind::Body)?, 0.0)?);
        blocks.push(Block::new(lines));
    }

    for (index, entry) in content.experiences.iter().enumerate() {
        let title = if german {
            "BERUFSERFAHRUNG"
        } else {
            "PROFESSIONAL EXPERIENCE"
        };
        let heading = format!("{} | {}", entry.title, entry.company);
        let mut span = date_span(
            entry.start_date.as_deref(),
            entry.end_date.as_deref(),
            german,
        );
        if let Some(location) = &entry.location {
            span = format!("{span} | {location}");
        }
        let mut lines = builder.section_prefix(index, title)?;
        lines.extend(builder.wrap(&heading, builder.style(Kind::Entry)?, 0.0)?);
        lines.extend(builder.wrap(&span, builder.style(Kind::Meta)?, 0.0)?);
        for bullet in &entry.bullets {
            lines.extend(builder.wrap_bullet(bullet)?);
        }
        lines.push(builder.spacer(4.0)?);
        blocks.push(Block::new(lines));
    }

    let project_title = if german { "PROJEKTE" } else { "PROJECTS" };
    for (index, entry) in content.additional.iter().enumerate() {
        let mut lines = builder.section_prefix(index, project_title)?;
        lines.extend(builder.wrap(&entry.title, builder.style(Kind::Entry)?, 0.0)?);
        if let Some(organization) = &entry.organization {
            lines.push(builder.fitted_link_line(
                organization,
                builder.style(Kind::Link)?,
                "projectUrl",
            )?);
        }
        for bullet in &entry.bullets {
            lines.extend(builder.wrap_bullet(bullet)?);
        }
        lines.push(builder.spacer(4.0)?);
        blocks.push(Block::new(lines));
    }

    let education_title = if german { "AUSBILDUNG" } else { "EDUCATION" };
    for (index, entry) in content.education.iter().enumerate() {
        let heading = format!("{} | {}", entry.degree, entry.institution);
        let mut span = date_span(
            entry.start_date.as_deref(),
            entry.end_date.as_deref(),
            german,
        );
        if let Some(location) = &entry.location {
            span = format!("{span} | {location}");
        }
        let mut lines = builder.section_prefix(index, education_title)?;
        lines.extend(builder.wrap(&heading, builder.style(Kind::Entry)?, 0.0)?);
        lines.extend(builder.wrap(&span, builder.style(Kind::Meta)?, 0.0)?);
        for bullet in &entry.bullets {
            lines.extend(builder.wrap_bullet(bullet)?);
        }
        lines.push(builder.spacer(4.0)?);
        blocks.push(Block::new(lines));
    }

    if !content.skill_groups.is_empty() {
        let mut lines = builder.section_prefix(0, if german { "KENNTNISSE" } else { "SKILLS" })?;
        for group in &content.skill_groups {
            let text = format!("{}: {}", group.category, group.skills.join(", "));
            lines.extend(builder.wrap(&text, builder.style(Kind::Body)?, 0.0)?);
        }
        lines.push(builder.spacer(4.0)?);
        blocks.push(Block::new(lines));
    }

    if !content.languages.is_empty() {
        let mut lines = builder.section_prefix(0, if german { "SPRACHEN" } else { "LANGUAGES" })?;
        let languages = content
            .languages
            .iter()
            .map(|row| match &row.proficiency {
                None => row.language.clone(),
                Some(proficiency) => format!("{} ({proficiency})", row.language),
            })
            .collect::<Vec<_>>()
            .join(", ");
        lines.extend(builder.wrap(&languages, builder.style(Kind::Body)?, 0.0)?);
        blocks.push(Block::new(lines));
    }

    Ok(GermanLayout {
        pages: builder.paginate(blocks)?,
        german_footer: german,
    })
}

/// Builds the German cover-letter layout.
///
/// # Errors
///
/// Returns a validation error for content that does not fit one or two pages, and
/// [`PdfError::Internal`] for an impossible measurement.
pub fn build_cover_letter_layout(
    content: &GermanCoverLetterContent,
) -> Result<GermanLayout, PdfError> {
    let builder = Builder::new(Document::CoverLetter);
    let mut blocks = vec![builder.header(&content.sender_name, &content.personal_fields)?];

    let mut recipient = vec![builder.spacer(8.0)?];
    if let Some(company) = &content.recipient_company {
        recipient.extend(builder.wrap(company, builder.style(Kind::Recipient)?, 0.0)?);
    }
    if let Some(location) = &content.recipient_location {
        recipient.extend(builder.wrap(location, builder.style(Kind::Meta)?, 0.0)?);
    }
    if recipient.len() > 1 {
        blocks.push(Block::new(recipient));
    }

    blocks.push(Block::new(vec![
        builder.spacer(12.0)?,
        line(&content.subject, builder.style(Kind::Subject)?, 0.0),
        builder.divider()?,
    ]));

    let body = builder.style(Kind::Body)?;
    let mut salutation = vec![builder.spacer(10.0)?];
    salutation.extend(builder.wrap(&content.salutation, body, 0.0)?);
    blocks.push(Block::new(salutation));

    for paragraph in &content.paragraphs {
        let mut lines = vec![builder.spacer(8.0)?];
        lines.extend(builder.wrap(paragraph, body, 0.0)?);
        blocks.push(Block::new(lines));
    }

    let mut closing = vec![builder.spacer(12.0)?];
    closing.extend(builder.wrap(&content.closing, body, 0.0)?);
    closing.push(builder.spacer(12.0)?);
    closing.extend(builder.wrap(&content.signature, builder.style(Kind::Signature)?, 0.0)?);
    blocks.push(Block::new(closing));

    Ok(GermanLayout {
        pages: builder.paginate(blocks)?,
        german_footer: true,
    })
}

/// Replaces control characters and characters that WinAnsi cannot encode.
///
/// Code points 0 to 31 and 127 to 159 become a space. Every other character that is not
/// Windows-1252 becomes `?`. Characters are counted by code point.
pub fn sanitize_german_pdf_text(text: &str) -> String {
    text.chars()
        .map(|character| {
            let code = u32::from(character);
            if code <= 31 || (127..=159).contains(&code) {
                ' '
            } else if encode_german_font_text(&character.to_string()).is_ok() {
                character
            } else {
                '?'
            }
        })
        .collect()
}

/// Layout rules for one document.
///
/// The sanitizer and the width measurements are the ones the caller sees; invalid input yields a
/// validation error with a fixed field and reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Builder {
    document: Document,
}

impl Builder {
    /// A builder for one document.
    pub const fn new(document: Document) -> Self {
        Self { document }
    }

    fn style(&self, kind: Kind) -> Result<Style, PdfError> {
        style_of(self.document, kind).ok_or(PdfError::Internal)
    }

    fn invalid(&self, field: &'static str, reason: &'static str) -> PdfError {
        let message = match self.document {
            Document::Resume => "Invalid German resume layout",
            Document::CoverLetter => "Invalid German cover-letter layout",
        };
        PdfError::Validation {
            message,
            field: Some(field),
            reason: Some(reason),
        }
    }

    fn tall_block_reason(&self) -> &'static str {
        match self.document {
            Document::Resume => "contains a resume block taller than one A4 page",
            Document::CoverLetter => "contains a cover-letter block taller than one A4 page",
        }
    }

    fn spacer(&self, height: f32) -> Result<RenderLine, PdfError> {
        Ok(RenderLine {
            text: String::new(),
            style: self.style(Kind::Spacer)?,
            indent: 0.0,
            height,
            font_size: 0.0,
        })
    }

    fn divider(&self) -> Result<RenderLine, PdfError> {
        Ok(line("", self.style(Kind::Divider)?, 0.0))
    }

    /// Width of text in points for a style, after sanitizing.
    pub fn width(&self, text: &str, style: Style) -> Result<f32, PdfError> {
        let sanitized = sanitize_german_pdf_text(text);
        let value = german_font_text_width(&sanitized, style.bold, style.font_size)
            .map_err(|_| PdfError::Internal)?;
        if !value.is_finite() || value < 0.0 {
            return Err(PdfError::Internal);
        }
        Ok(value)
    }

    /// Greedy wrapping of text to the content width.
    ///
    /// Words break on ASCII whitespace. A word wider than the line is split at code points.
    /// The resume keeps one empty row for empty text, the cover letter keeps none.
    ///
    /// # Errors
    ///
    /// Returns [`PdfError::Internal`] only when the font measurement is impossible.
    pub fn wrap(&self, text: &str, style: Style, indent: f32) -> Result<Vec<RenderLine>, PdfError> {
        let sanitized = sanitize_german_pdf_text(text);
        let safe = java_strip(&sanitized);
        let available = CONTENT_WIDTH - indent;
        let mut lines = Vec::new();
        let mut current = String::new();
        let mut current_units = 0.0_f32;
        for word in words(safe) {
            let separator = if current.is_empty() { "" } else { " " };
            let units = advance(current_units, separator, style.bold)?;
            let candidate_units = advance(units, word, style.bold)?;
            if scaled_width(candidate_units, style) <= available {
                if current.is_empty() {
                    current.push_str(word);
                } else {
                    current.push(' ');
                    current.push_str(word);
                }
                current_units = candidate_units;
                continue;
            }
            if !current.is_empty() {
                lines.push(line(&current, style, indent));
            }
            let word_units = if units != 0.0 {
                advance(0.0, word, style.bold)?
            } else {
                candidate_units
            };
            if scaled_width(word_units, style) <= available {
                current = word.to_owned();
                current_units = word_units;
                continue;
            }
            // Per-glyph accumulation restarts at each emitted fragment.
            let characters: Vec<char> = word.chars().collect();
            let mut start = 0_usize;
            let mut fragment_units = 0.0_f32;
            for (index, character) in characters.iter().copied().enumerate() {
                let mut buffer = [0_u8; 4];
                let glyph = character.encode_utf8(&mut buffer);
                let mut next_units = advance(fragment_units, glyph, style.bold)?;
                if index > start && scaled_width(next_units, style) > available {
                    let fragment: String = characters[start..index].iter().collect();
                    lines.push(line(&fragment, style, indent));
                    start = index;
                    next_units = advance(0.0, glyph, style.bold)?;
                }
                fragment_units = next_units;
            }
            current = characters[start..].iter().collect();
            current_units = fragment_units;
        }
        if !current.is_empty() || self.document == Document::Resume {
            lines.push(line(&current, style, indent));
        }
        Ok(lines)
    }

    /// A bullet: the first row uses the bullet style, continuation rows the body style, all at
    /// indent 12.
    pub fn wrap_bullet(&self, text: &str) -> Result<Vec<RenderLine>, PdfError> {
        let wrapped = self.wrap(text, self.style(Kind::Bullet)?, 12.0)?;
        let body = self.style(Kind::Body)?;
        let mut rows = Vec::with_capacity(wrapped.len());
        for (index, row) in wrapped.into_iter().enumerate() {
            if index == 0 {
                rows.push(row);
            } else {
                rows.push(RenderLine {
                    text: row.text,
                    style: body,
                    indent: row.indent,
                    height: row.height,
                    font_size: body.font_size,
                });
            }
        }
        Ok(rows)
    }

    /// A single row whose font shrinks to fit the content width.
    ///
    /// # Errors
    ///
    /// Returns the validation error `must fit on one readable line` when the shrunk size would
    /// fall below 7 points.
    pub fn fitted_link_line(
        &self,
        text: &str,
        style: Style,
        field: &'static str,
    ) -> Result<RenderLine, PdfError> {
        let sanitized = sanitize_german_pdf_text(text);
        let width = self.width(&sanitized, style)?;
        let size = if width <= CONTENT_WIDTH {
            style.font_size
        } else {
            (style.font_size * CONTENT_WIDTH) / width
        };
        if size < 7.0 {
            return Err(self.invalid(field, "must fit on one readable line"));
        }
        Ok(RenderLine {
            text: sanitized,
            style,
            indent: 0.0,
            height: style.leading,
            font_size: size,
        })
    }

    fn contact_lines(&self, field: &PersonalField) -> Result<Vec<RenderLine>, PdfError> {
        let text = format!("{}: {}", field.label, field.value);
        let is_url = match self.document {
            Document::Resume => {
                let lower = field.value.to_lowercase();
                lower.starts_with("http://") || lower.starts_with("https://")
            }
            Document::CoverLetter => {
                // Per-character upper then lower, over the first 8 characters.
                let prefix: String = field
                    .value
                    .chars()
                    .take(8)
                    .flat_map(char::to_uppercase)
                    .flat_map(char::to_lowercase)
                    .collect();
                prefix.starts_with("http://") || prefix.starts_with("https://")
            }
        };
        let style = self.style(Kind::Contact)?;
        if is_url {
            Ok(vec![self.fitted_link_line(&text, style, "contactUrl")?])
        } else {
            self.wrap(&text, style, 0.0)
        }
    }

    /// The header block: name, divider and contact fields paired where they fit.
    pub fn header(&self, name: &str, fields: &[PersonalField]) -> Result<Block, PdfError> {
        let mut lines = self.wrap(name, self.style(Kind::Name)?, 0.0)?;
        lines.push(self.divider()?);
        let contact = self.style(Kind::Contact)?;
        let mut index = 0;
        while index < fields.len() {
            let first = &fields[index];
            let mut pair = format!("{}: {}", first.label, first.value);
            if let Some(second) = fields.get(index + 1) {
                pair.push_str(&format!("  |  {}: {}", second.label, second.value));
            }
            if self.width(&pair, contact)? <= CONTENT_WIDTH {
                lines.push(line(&pair, contact, 0.0));
                index += 2;
            } else {
                lines.extend(self.contact_lines(first)?);
                index += 1;
            }
        }
        Ok(Block::new(lines))
    }

    /// The section heading of the first entry of a group: a spacer and the heading row. Later
    /// entries of the group have no heading.
    pub fn section_prefix(&self, index: usize, title: &str) -> Result<Vec<RenderLine>, PdfError> {
        if index != 0 {
            return Ok(Vec::new());
        }
        Ok(vec![
            self.spacer(4.0)?,
            line(title, self.style(Kind::Section)?, 0.0),
        ])
    }

    /// Splits blocks over one or two pages.
    ///
    /// A resume with two pages moves blocks from the first page to the second until the last page
    /// holds [`MIN_LAST_PAGE_USAGE`], while the first page keeps at least that much height.
    ///
    /// # Errors
    ///
    /// Returns a validation error for a block taller than a page, for content that needs more
    /// than two pages, and for a resume whose last page stays mostly empty.
    pub fn paginate(&self, blocks: Vec<Block>) -> Result<Vec<PageLayout>, PdfError> {
        let mut pages: Vec<PageLayout> = Vec::new();
        let mut current: Vec<Block> = Vec::new();
        let mut used = 0.0_f32;
        for block in blocks {
            let height = block.height();
            if height > PAGE_CAPACITY {
                return Err(self.invalid("content", self.tall_block_reason()));
            }
            if !current.is_empty() && used + height > PAGE_CAPACITY {
                pages.push(PageLayout {
                    blocks: std::mem::take(&mut current),
                    used_height: used,
                });
                used = 0.0;
            }
            current.push(block);
            used += height;
        }
        pages.push(PageLayout {
            blocks: current,
            used_height: used,
        });

        if pages.len() > 2 {
            let reason = match self.document {
                Document::CoverLetter => "must fit within one or two A4 pages",
                Document::Resume => "must fit within two A4 resume pages",
            };
            return Err(self.invalid("content", reason));
        }
        if self.document == Document::Resume && pages.len() == 2 {
            let mut iter = pages.into_iter();
            let (Some(first), Some(last)) = (iter.next(), iter.next()) else {
                return Err(PdfError::Internal);
            };
            let (mut first_blocks, mut first_used) = (first.blocks, first.used_height);
            let (mut last_blocks, mut last_used) = (last.blocks, last.used_height);
            while last_used < MIN_LAST_PAGE_USAGE {
                let height = match first_blocks.last() {
                    Some(candidate) => candidate.height(),
                    None => return Err(PdfError::Internal),
                };
                if last_used + height > PAGE_CAPACITY || first_used - height < MIN_LAST_PAGE_USAGE {
                    break;
                }
                let Some(candidate) = first_blocks.pop() else {
                    return Err(PdfError::Internal);
                };
                last_blocks.insert(0, candidate);
                first_used -= height;
                last_used += height;
            }
            if last_used < MIN_LAST_PAGE_USAGE {
                return Err(self.invalid(
                    "content",
                    "would create a mostly empty trailing resume page",
                ));
            }
            return Ok(vec![
                PageLayout {
                    blocks: first_blocks,
                    used_height: first_used,
                },
                PageLayout {
                    blocks: last_blocks,
                    used_height: last_used,
                },
            ]);
        }
        Ok(pages)
    }
}

/// Words of a stripped text, split on ASCII whitespace. Empty text is one empty word.
fn words(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return vec![""];
    }
    text.split([' ', '\t', '\n', '\x0b', '\x0c', '\r'])
        .filter(|word| !word.is_empty())
        .collect()
}

/// A row with sanitized text.
fn line(text: &str, style: Style, indent: f32) -> RenderLine {
    RenderLine {
        text: sanitize_german_pdf_text(text),
        style,
        indent,
        height: style.leading,
        font_size: style.font_size,
    }
}

/// The advance of one glyph at size 1000, which must be a positive integer.
fn glyph_units(character: char, bold: bool) -> Result<f32, PdfError> {
    let mut buffer = [0_u8; 4];
    let width = german_font_text_width(character.encode_utf8(&mut buffer), bold, 1000.0)
        .map_err(|_| PdfError::Internal)?;
    if !width.is_finite() || width <= 0.0 || width.fract() != 0.0 {
        return Err(PdfError::Internal);
    }
    Ok(width)
}

/// Adds the advances of the characters of `text` to `units`, one binary32 rounding per glyph.
fn advance(units: f32, text: &str, bold: bool) -> Result<f32, PdfError> {
    let mut total = units;
    for character in text.chars() {
        total += glyph_units(character, bold)?;
    }
    Ok(total)
}

/// Width in points of an advance in 1000-unit space: `f32(f32(units / 1000) * size)`.
fn scaled_width(units: f32, style: Style) -> f32 {
    (units / 1000.0) * style.font_size
}

/// The date span of an entry, `MM/YYYY - MM/YYYY`, with the unknown and present labels.
/// Dates are sliced by code point.
fn date_span(start: Option<&str>, end: Option<&str>, german: bool) -> String {
    let (unknown, present) = if german {
        ("unbekannt", "heute")
    } else {
        ("Unknown", "Present")
    };
    if start.is_none() && end.is_none() {
        return unknown.to_owned();
    }
    let month = |value: &str| -> String {
        let slice = |from: usize, to: usize| -> String {
            value
                .chars()
                .skip(from)
                .take(to.saturating_sub(from))
                .collect()
        };
        format!("{}/{}", slice(5, 7), slice(0, 4))
    };
    let left = start.map_or_else(|| unknown.to_owned(), month);
    let right = end.map_or_else(|| present.to_owned(), month);
    format!("{left} - {right}")
}

/// Exact sum of finite doubles, rounded once (Shewchuk partials).
fn fsum(values: impl IntoIterator<Item = f64>) -> f64 {
    let mut partials: Vec<f64> = Vec::new();
    for value in values {
        let mut x = value;
        let mut kept = 0;
        for index in 0..partials.len() {
            let mut y = partials[index];
            if x.abs() < y.abs() {
                std::mem::swap(&mut x, &mut y);
            }
            let hi = x + y;
            let low = y - (hi - x);
            if low != 0.0 {
                partials[kept] = low;
                kept += 1;
            }
            x = hi;
        }
        partials.truncate(kept);
        partials.push(x);
    }

    let mut count = partials.len();
    let mut hi = 0.0_f64;
    if count > 0 {
        count -= 1;
        hi = partials[count];
        let mut low = 0.0_f64;
        while count > 0 {
            let x = hi;
            count -= 1;
            let y = partials[count];
            hi = x + y;
            let yr = hi - x;
            low = y - yr;
            if low != 0.0 {
                break;
            }
        }
        if count > 0
            && ((low < 0.0 && partials[count - 1] < 0.0)
                || (low > 0.0 && partials[count - 1] > 0.0))
        {
            let y = low * 2.0;
            let x = hi + y;
            let yr = x - hi;
            if y == yr {
                hi = x;
            }
        }
    }
    hi
}

#[cfg(test)]
mod tests {
    use super::{fsum, sanitize_german_pdf_text};

    #[test]
    fn fsum_is_exact_where_naive_addition_is_not() {
        assert_eq!(fsum([0.1, 0.2, 0.3]), 0.6);
        assert_eq!(fsum([1e16, 1.0, -1e16]), 1.0);
    }

    #[test]
    fn sanitizer_keeps_windows_1252_and_maps_the_rest() {
        assert_eq!(sanitize_german_pdf_text("a\tb\u{7f}"), "a b ");
        assert_eq!(sanitize_german_pdf_text("€ ß \u{2028}"), "€ ß ?");
    }
}
