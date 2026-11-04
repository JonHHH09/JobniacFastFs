//! A seeded synthetic corpus of labelled resumes and cover letters.
//!
//! Every document is generated from a seed alone (a SplitMix64 stream), so the same seed gives
//! byte-identical output. All content is fictional: people are a common first name with an invented
//! surname, companies are `Example`/`Sample`/`Demo` names, emails are on `example.com` or
//! `example.org`, URLs are on those domains, and phone numbers are NANP `555-01xx` numbers, the
//! ranges reserved for fiction (Germany `030 23125`, France `01 99 00`) or obviously invalid
//! numbers (`+355 69 000 00xx`, for countries without a reserved range).
//!
//! Each document is emitted twice from one layout: as plain text lines and as geometry lines with
//! synthesized coordinates and fonts. Both lists hold the same raw lines in reading order with the
//! same labels. Styles: US chronological, Canadian (English, French), Europass two-column
//! (English, German, Albanian, French), German Lebenslauf; and cover letters for the US, Canada
//! (French), Europass, German DIN and Albanian layouts.
//!
//! # Variation, so that labels cannot be learned from lexicon flags alone
//!
//! * About a third of the headings, employers, titles, degrees, salutations, closings and subjects
//!   come from held-out pools (`pools.rs`) that are not in the feature lexicons.
//! * Sections are optional and reordered (summary, skills with 0 to 4 lines, languages, projects,
//!   certifications, publications, volunteering, references, interests); 1 to 5 jobs and 1 to 3
//!   education entries; contact lines in varying order and place (after the name, before it, in a
//!   one-line band, or as a footer); entries in four layouts (right-aligned dates, stacked,
//!   organization first, inline).
//! * Typography varies per document: body 9 to 11 pt, headings 10.5 to 14 pt set bold, in capitals
//!   or plain, name 14 to 24 pt (14 to 16 pt in a sidebar).
//! * Dates use several formats per style and language (names, numeric, ISO), several separators
//!   (dashes with and without spaces, `to`, `bis`, `deri`, `à`) and ongoing markers in every
//!   language, sometimes in capitals.
//! * Noise: page numbers and footers, stray single words, document titles (`CV`), merged lines,
//!   addresses wrapped after a comma and URLs wrapped at the right margin.
//!
//! # Label semantics (the same in every style)
//!
//! * A line that merges several parts takes the role of its first part: `Title at Org, City` is a
//!   Title; `Org, City` is an Organization (a city that shares the organization line belongs to it);
//!   a city alone on a line, or right-aligned as its own fragment, is a Location.
//! * Right-aligned dates and cities are separate lines (fragments) on the same row.
//! * Contact covers addresses, phones, emails and links, also when joined with `|` on one line.
//! * Skill, language and interest lines, summary text and references are Body; page numbers,
//!   footers, stray words and document titles are Other.
//! * A wrapped fragment keeps the role of the line it continues and is flagged
//!   [`LabelledLine::continues`]; the reading pipeline joins it, and [`LabelledDocument::pipeline_view`]
//!   gives the expected joined lines with their labels.

mod data;
mod letter;
mod pools;
mod resume;

use super::labels::{LetterLineRole, LineLabel, ResumeLineRole};
use super::lexicon::Language;
use super::model::{LineGeometry, TextLine};

/// The small, fast SplitMix64 generator the corpus uses.
#[derive(Debug, Clone)]
pub struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    /// A generator from a seed.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// The next 64 random bits.
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number in `0..bound` (`bound` of 0 gives 0).
    pub fn below(&mut self, bound: usize) -> usize {
        if bound == 0 {
            return 0;
        }
        usize::try_from(self.next_u64() % bound as u64).unwrap_or(0)
    }

    /// A number in `low..=high`.
    pub fn range(&mut self, low: i32, high: i32) -> i32 {
        let span = usize::try_from(high - low + 1).unwrap_or(1);
        low + i32::try_from(self.below(span)).unwrap_or(0)
    }

    /// True with the given chance in per cent.
    pub fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }

    /// One element of a non-empty slice.
    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

/// Whether a document is a resume or a cover letter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DocumentKind {
    /// A resume or CV.
    Resume,
    /// A cover letter.
    CoverLetter,
}

/// A layout and language convention of the corpus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CorpusStyle {
    /// US chronological resume, English.
    UsChronological,
    /// Canadian resume, English.
    CanadaEnglish,
    /// Canadian resume, French.
    CanadaFrench,
    /// Europass two-column resume, English.
    EuropassEnglish,
    /// Europass two-column resume, German.
    EuropassGerman,
    /// Europass two-column resume, Albanian.
    EuropassAlbanian,
    /// Europass two-column resume, French.
    EuropassFrench,
    /// German Lebenslauf, single column.
    GermanLebenslauf,
    /// US business letter, English.
    LetterUs,
    /// Canadian business letter, French.
    LetterCanadaFrench,
    /// Europass cover letter, English.
    LetterEuropass,
    /// German DIN-style letter.
    LetterGermanDin,
    /// Albanian letter.
    LetterAlbanian,
}

impl CorpusStyle {
    /// The resume styles.
    pub const RESUMES: [CorpusStyle; 8] = [
        CorpusStyle::UsChronological,
        CorpusStyle::CanadaEnglish,
        CorpusStyle::CanadaFrench,
        CorpusStyle::EuropassEnglish,
        CorpusStyle::EuropassGerman,
        CorpusStyle::EuropassAlbanian,
        CorpusStyle::EuropassFrench,
        CorpusStyle::GermanLebenslauf,
    ];

    /// The cover-letter styles.
    pub const LETTERS: [CorpusStyle; 5] = [
        CorpusStyle::LetterUs,
        CorpusStyle::LetterCanadaFrench,
        CorpusStyle::LetterEuropass,
        CorpusStyle::LetterGermanDin,
        CorpusStyle::LetterAlbanian,
    ];

    /// The stable name of the style.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            CorpusStyle::UsChronological => "us_chronological",
            CorpusStyle::CanadaEnglish => "canada_english",
            CorpusStyle::CanadaFrench => "canada_french",
            CorpusStyle::EuropassEnglish => "europass_english",
            CorpusStyle::EuropassGerman => "europass_german",
            CorpusStyle::EuropassAlbanian => "europass_albanian",
            CorpusStyle::EuropassFrench => "europass_french",
            CorpusStyle::GermanLebenslauf => "german_lebenslauf",
            CorpusStyle::LetterUs => "letter_us",
            CorpusStyle::LetterCanadaFrench => "letter_canada_french",
            CorpusStyle::LetterEuropass => "letter_europass",
            CorpusStyle::LetterGermanDin => "letter_german_din",
            CorpusStyle::LetterAlbanian => "letter_albanian",
        }
    }

    /// Resume or cover letter.
    #[must_use]
    pub fn kind(self) -> DocumentKind {
        if Self::LETTERS.contains(&self) {
            DocumentKind::CoverLetter
        } else {
            DocumentKind::Resume
        }
    }

    /// The language of the style.
    #[must_use]
    pub fn language(self) -> Language {
        match self {
            CorpusStyle::UsChronological
            | CorpusStyle::CanadaEnglish
            | CorpusStyle::EuropassEnglish
            | CorpusStyle::LetterUs
            | CorpusStyle::LetterEuropass => Language::En,
            CorpusStyle::CanadaFrench
            | CorpusStyle::EuropassFrench
            | CorpusStyle::LetterCanadaFrench => Language::Fr,
            CorpusStyle::EuropassGerman
            | CorpusStyle::GermanLebenslauf
            | CorpusStyle::LetterGermanDin => Language::De,
            CorpusStyle::EuropassAlbanian | CorpusStyle::LetterAlbanian => Language::Sq,
        }
    }
}

/// One labelled line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelledLine {
    /// The line.
    pub line: TextLine,
    /// Its role.
    pub label: LineLabel,
    /// `Some(separator)` when this raw line is a wrapped fragment of the previous one (a URL tail,
    /// separator `""`, or the rest of an address, separator `" "`), which the reading pipeline
    /// joins to it.
    pub continues: Option<&'static str>,
}

/// One generated document: the same labelled lines as plain text and as geometry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelledDocument {
    /// The style it was generated in.
    pub style: CorpusStyle,
    /// Resume or letter.
    pub kind: DocumentKind,
    /// The lines without geometry, with `blank_before` set where a paragraph gap is.
    pub plain: Vec<LabelledLine>,
    /// The same lines with synthesized geometry, in reading order.
    pub geometry: Vec<LabelledLine>,
}

impl LabelledDocument {
    /// The plain text of the document: lines joined with newlines, a blank line where a paragraph
    /// gap is.
    #[must_use]
    pub fn plain_text(&self) -> String {
        let mut text = String::new();
        for (position, labelled) in self.plain.iter().enumerate() {
            if position > 0 {
                text.push('\n');
                if labelled.line.blank_before {
                    text.push('\n');
                }
            }
            text.push_str(&labelled.line.text);
        }
        text
    }

    /// The label of each raw line, in reading order.
    #[must_use]
    pub fn labels(&self) -> Vec<LineLabel> {
        self.plain.iter().map(|labelled| labelled.label).collect()
    }

    /// The lines the reading pipeline should produce, with their labels: the raw lines with every
    /// wrapped fragment joined to the line it continues.
    #[must_use]
    pub fn pipeline_view(&self) -> Vec<(String, LineLabel)> {
        let mut joined: Vec<(String, LineLabel)> = Vec::with_capacity(self.plain.len());
        for labelled in &self.plain {
            match (labelled.continues, joined.last_mut()) {
                (Some(separator), Some((text, _))) => {
                    text.push_str(separator);
                    text.push_str(&labelled.line.text);
                }
                _ => joined.push((labelled.line.text.clone(), labelled.label)),
            }
        }
        joined
    }

    /// The geometry lines without their labels.
    #[must_use]
    pub fn geometry_lines(&self) -> Vec<TextLine> {
        self.geometry
            .iter()
            .map(|labelled| labelled.line.clone())
            .collect()
    }
}

/// The content pools of a language.
fn lang_data(language: Language) -> &'static data::LangData {
    match language {
        Language::En => &data::EN,
        Language::Fr => &data::FR,
        Language::De => &data::DE,
        Language::Sq => &data::SQ,
    }
}

/// The held-out pools of a language.
fn lang_pools(language: Language) -> &'static pools::Pools {
    match language {
        Language::En => &pools::EN,
        Language::Fr => &pools::FR,
        Language::De => &pools::DE,
        Language::Sq => &pools::SQ,
    }
}

/// Generates one document of `style` from `seed`.
#[must_use]
pub fn synthetic_document(style: CorpusStyle, seed: u64) -> LabelledDocument {
    let mut rng = SplitMix64::new(seed);
    let _warm_up = rng.next_u64();
    let (page, mut items) = match style.kind() {
        DocumentKind::Resume => resume::build(style, &mut rng),
        DocumentKind::CoverLetter => letter::build(style, &mut rng),
    };
    if rng.chance(50) {
        items = add_footers(&mut rng, style, &page, items);
    }
    render(style, &page, &items)
}

/// The seed of document `index` of a corpus seeded with `seed`.
fn document_seed(seed: u64, index: usize) -> u64 {
    let mut mixer = SplitMix64::new(seed ^ (index as u64).wrapping_mul(0xD6E8_FEB8_6659_FD93));
    mixer.next_u64()
}

/// `count` resumes, styles in turn (`index % 8`), each from its own sub-seed of `seed`.
#[must_use]
pub fn synthetic_resumes(seed: u64, count: usize) -> Vec<LabelledDocument> {
    (0..count)
        .map(|index| {
            let style = CorpusStyle::RESUMES[index % CorpusStyle::RESUMES.len()];
            synthetic_document(style, document_seed(seed, index))
        })
        .collect()
}

/// `count` cover letters, styles in turn (`index % 5`), each from its own sub-seed of `seed`.
#[must_use]
pub fn synthetic_cover_letters(seed: u64, count: usize) -> Vec<LabelledDocument> {
    (0..count)
        .map(|index| {
            let style = CorpusStyle::LETTERS[index % CorpusStyle::LETTERS.len()];
            synthetic_document(style, document_seed(seed, index))
        })
        .collect()
}

// ---- layout ----

/// Horizontal alignment of a full-width line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Align {
    Center,
    Right,
}

/// Where an item is placed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    /// The main column (the only column of a single-column page).
    Main,
    /// The narrow sidebar column.
    Side,
    /// Across the page.
    Full(Align),
    /// Right-aligned on the row of the previous item.
    RightOfPrev,
    /// Centered in the bottom margin of the given page.
    Footer(u32),
}

/// One line to place and label.
#[derive(Debug, Clone)]
struct Item {
    text: String,
    label: LineLabel,
    size: i32,
    bold: bool,
    italic: bool,
    slot: Slot,
    gap: bool,
    continues: Option<&'static str>,
    full_width: bool,
}

impl Item {
    fn resume(text: impl Into<String>, role: ResumeLineRole, slot: Slot) -> Self {
        Self::new(text, LineLabel::Resume(role), slot)
    }

    fn letter(text: impl Into<String>, role: LetterLineRole, slot: Slot) -> Self {
        Self::new(text, LineLabel::Letter(role), slot)
    }

    fn new(text: impl Into<String>, label: LineLabel, slot: Slot) -> Self {
        Self {
            text: text.into(),
            label,
            size: 1000,
            bold: false,
            italic: false,
            slot,
            gap: false,
            continues: None,
            full_width: false,
        }
    }

    fn size(mut self, size: i32) -> Self {
        self.size = size;
        self
    }

    fn bold(mut self) -> Self {
        self.bold = true;
        self
    }

    fn italic(mut self) -> Self {
        self.italic = true;
        self
    }

    fn gap(mut self) -> Self {
        self.gap = true;
        self
    }

    /// Marks the item as a wrapped fragment of the previous line.
    fn continues(mut self, separator: &'static str) -> Self {
        self.continues = Some(separator);
        self
    }

    /// Makes the line box reach the right edge of its column (a line wrapped at the margin).
    fn full_width(mut self) -> Self {
        self.full_width = true;
        self
    }
}

/// The page box and column positions of a style, in centipoints.
#[derive(Debug, Clone, Copy)]
struct Page {
    width: i32,
    height: i32,
    margin_top: i32,
    margin_bottom: i32,
    side_x0: i32,
    main_x0: i32,
    main_x1: i32,
}

impl Page {
    fn single_column(width: i32, height: i32) -> Self {
        let margin_x = 5400;
        Self {
            width,
            height,
            margin_top: 5400,
            margin_bottom: 5400,
            side_x0: margin_x,
            main_x0: margin_x,
            main_x1: width - margin_x,
        }
    }

    /// A page with a narrow left sidebar (about 28 % of the width) and a main column.
    fn sidebar(width: i32, height: i32) -> Self {
        Self {
            width,
            height,
            margin_top: 4800,
            margin_bottom: 4800,
            side_x0: 2800,
            main_x0: 21000,
            main_x1: width - 3000,
        }
    }

    /// The characters of a font size that fit the main column (with a 10 % reserve).
    fn main_capacity(&self, size: i32) -> usize {
        let width = i64::from(self.main_x1 - self.main_x0);
        usize::try_from(width * 2 / i64::from(size.max(1)) * 9 / 10).unwrap_or(40)
    }
}

/// The line height (advance) and the box height of a font size.
fn advance(size: i32) -> i32 {
    size * 13 / 10
}

fn box_height(size: i32) -> i32 {
    size * 12 / 10
}

fn estimated_width(text: &str, size: i32) -> i32 {
    i32::try_from(text.chars().count()).unwrap_or(0) * size / 2
}

/// Where one item landed.
#[derive(Debug, Clone, Copy)]
struct Placed {
    page: u32,
    x0: i32,
    y0: i32,
    x1: i32,
}

/// Places the items on pages.
fn place(page: &Page, items: &[Item]) -> Vec<Placed> {
    let mut placed = Vec::with_capacity(items.len());
    let mut page_number = 0_u32;
    let (mut main_y, mut side_y) = (page.margin_top, page.margin_top);
    let mut previous: Option<(u32, i32)> = None;
    for item in items {
        let width = estimated_width(&item.text, item.size);
        let extra = if item.gap { item.size * 6 / 10 } else { 0 };
        let (item_page, x0, y0) = match item.slot {
            Slot::Footer(footer_page) => {
                let y = page.height - page.margin_bottom + 600;
                placed.push(Placed {
                    page: footer_page,
                    x0: (page.width - width) / 2,
                    y0: y,
                    x1: (page.width - width) / 2 + width,
                });
                continue;
            }
            Slot::RightOfPrev => {
                let (previous_page, previous_y) = previous.unwrap_or((0, page.margin_top));
                page_number = previous_page;
                (page_number, page.main_x1 - width, previous_y)
            }
            slot => {
                let cursor = match slot {
                    Slot::Side => side_y,
                    Slot::Main => main_y,
                    _ => main_y.max(side_y),
                };
                let mut y = if cursor == page.margin_top {
                    cursor
                } else {
                    cursor + extra
                };
                if y + box_height(item.size) > page.height - page.margin_bottom {
                    page_number += 1;
                    main_y = page.margin_top;
                    side_y = page.margin_top;
                    y = page.margin_top;
                }
                let x = match slot {
                    Slot::Side => page.side_x0,
                    Slot::Main => page.main_x0,
                    Slot::Full(Align::Center) => (page.width - width) / 2,
                    _ => page.main_x1 - width,
                };
                let next = y + advance(item.size);
                match slot {
                    Slot::Side => side_y = next,
                    Slot::Main => main_y = next,
                    _ => {
                        main_y = next;
                        side_y = next;
                    }
                }
                (page_number, x, y)
            }
        };
        previous = Some((item_page, y0));
        let x1 = if item.full_width && item.slot == Slot::Main {
            page.main_x1.max(x0 + width)
        } else {
            x0 + width
        };
        placed.push(Placed {
            page: item_page,
            x0,
            y0,
            x1,
        });
    }
    placed
}

/// Page footers (`Page 1/2`, `1`, `- 1 -`) after the last line of every page.
fn add_footers(
    rng: &mut SplitMix64,
    style: CorpusStyle,
    page: &Page,
    items: Vec<Item>,
) -> Vec<Item> {
    let placed = place(page, &items);
    let pages = placed.iter().map(|p| p.page).max().map_or(1, |m| m + 1);
    let words = lang_pools(style.language()).page_words;
    let variant = rng.below(4);
    let other = match style.kind() {
        DocumentKind::Resume => LineLabel::Resume(ResumeLineRole::Other),
        DocumentKind::CoverLetter => LineLabel::Letter(LetterLineRole::Other),
    };
    let mut result = Vec::with_capacity(items.len() + 2);
    for (position, item) in items.into_iter().enumerate() {
        result.push(item);
        let current = placed[position].page;
        let last_of_page = placed
            .get(position + 1)
            .is_none_or(|next| next.page != current);
        if last_of_page {
            let number = current + 1;
            let text = match variant {
                0 => format!("{} {number}/{pages}", words[0]),
                1 => number.to_string(),
                2 => format!("{} {number} {} {pages}", words[0], words[1]),
                _ => format!("- {number} -"),
            };
            result.push(
                Item::new(text, other, Slot::Footer(current))
                    .size(800)
                    .gap(),
            );
        }
    }
    result
}

/// Builds the plain and geometry line lists.
fn render(style: CorpusStyle, page: &Page, items: &[Item]) -> LabelledDocument {
    let placed = place(page, items);
    let mut plain = Vec::with_capacity(items.len());
    let mut geometry = Vec::with_capacity(items.len());
    for (position, (item, spot)) in items.iter().zip(&placed).enumerate() {
        let index = u32::try_from(position).unwrap_or(u32::MAX);
        let mut plain_line = TextLine::new(0, index, &item.text, None);
        plain_line.blank_before = item.gap && position > 0;
        plain.push(LabelledLine {
            line: plain_line,
            label: item.label,
            continues: item.continues,
        });
        geometry.push(LabelledLine {
            line: TextLine::new(
                spot.page,
                index,
                &item.text,
                Some(LineGeometry {
                    x0: spot.x0,
                    y0: spot.y0,
                    x1: spot.x1,
                    y1: spot.y0 + box_height(item.size),
                    font_size_centipoints: item.size,
                    bold: item.bold,
                    italic: item.italic,
                    page_width_centipoints: page.width,
                    page_height_centipoints: page.height,
                }),
            ),
            label: item.label,
            continues: item.continues,
        });
    }
    LabelledDocument {
        style,
        kind: style.kind(),
        plain,
        geometry,
    }
}

// ---- shared helpers ----

/// Greedy word wrap at `width` characters.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if !current.is_empty() && current.chars().count() + 1 + word.chars().count() > width {
            lines.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

/// Lower-case ASCII of a name for an email or URL.
fn ascii_lower(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            'à' | 'á' | 'â' | 'ä' | 'À' => 'a',
            'é' | 'è' | 'ê' | 'ë' | 'É' => 'e',
            'î' | 'ï' => 'i',
            'ô' | 'ö' => 'o',
            'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            other => other.to_ascii_lowercase(),
        })
        .collect()
}

/// A fictional person.
struct Person {
    first: String,
    last: String,
    email: String,
    /// `https://www.example.com/in/first-last`.
    url_long: String,
    /// `https://example.org/first`.
    url_short: String,
}

impl Person {
    fn new(rng: &mut SplitMix64, data: &data::LangData) -> Self {
        let first = (*rng.pick(data.first_names)).to_owned();
        let last = (*rng.pick(data.last_names)).to_owned();
        let (f, l) = (ascii_lower(&first), ascii_lower(&last));
        let domain = if rng.chance(50) {
            "example.com"
        } else {
            "example.org"
        };
        Self {
            email: format!("{f}.{l}@{domain}"),
            url_long: format!("https://www.example.com/in/{f}-{l}"),
            url_short: format!("https://example.org/{f}"),
            first,
            last,
        }
    }

    fn full_name(&self) -> String {
        format!("{} {}", self.first, self.last)
    }
}

/// A fictional company name: with a legal suffix of the language, or (about a third of the time)
/// a held-out name without one.
fn company(rng: &mut SplitMix64, language: Language) -> String {
    let data = lang_data(language);
    if rng.chance(33) {
        return (*rng.pick(lang_pools(language).held_employers)).to_owned();
    }
    format!(
        "{} {} {}",
        rng.pick(data::COMPANY_STEMS),
        rng.pick(data::COMPANY_NOUNS),
        rng.pick(data.company_suffixes)
    )
}

/// One entry of `standard`, or (about a third of the time) of `held_out`.
fn pick_held(
    rng: &mut SplitMix64,
    standard: &[&'static str],
    held_out: &[&'static str],
) -> &'static str {
    if rng.chance(33) && !held_out.is_empty() {
        rng.pick(held_out)
    } else {
        rng.pick(standard)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitmix_matches_the_reference_stream() {
        let mut rng = SplitMix64::new(0);
        assert_eq!(rng.next_u64(), 0xE220_A839_7B1D_CDAF);
        assert_eq!(rng.next_u64(), 0x6E78_9E6A_A1B9_65F4);
    }

    #[test]
    fn wrap_breaks_on_words() {
        assert_eq!(wrap("aa bb cc dd", 5), vec!["aa bb", "cc dd"]);
        assert_eq!(wrap("", 5), Vec::<String>::new());
    }
}
