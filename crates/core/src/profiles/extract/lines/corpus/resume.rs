//! Synthetic resumes: US, Canadian, Europass two-column and German Lebenslauf layouts, with varied
//! sections, entry layouts, typography, dates and noise (see the corpus module docs).

use super::data::{
    CA_CITIES, DE_CITIES, DE_STREETS, EN_STREETS, FR_CITIES, FR_STREETS, SQ_CITIES, SQ_STREETS,
    US_CITIES,
};
use super::pools::STRAY_WORDS;
use super::{
    Align, CorpusStyle, Item, Page, Person, Slot, SplitMix64, company, lang_data, lang_pools,
    pick_held, wrap,
};
use crate::profiles::extract::lines::labels::ResumeLineRole as Role;
use crate::profiles::extract::lines::lexicon::{Language, SectionKind};

/// The place conventions of a style.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Region {
    Us,
    Ca,
    CaFr,
    De,
    Sq,
    Fr,
}

/// How months are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DateStyle {
    Abbreviated,
    Long,
    Slash,
    Dot,
    Iso,
}

/// How a job or study entry is laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryLayout {
    /// Organization with the city right-aligned, title with the dates right-aligned.
    RightAligned,
    /// Dates, title, then `Organization, City`.
    Stacked,
    /// Organization, title, dates, and the city on its own line or on the organization line.
    OrgFirst,
    /// `Title at Organization, City` (or `Title | Dates`), then the other line.
    Inline,
}

/// How headings are set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HeadingMode {
    Bold,
    Caps,
    Plain,
}

/// Where the contact lines go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContactPlace {
    AfterName,
    BeforeName,
    Band,
    Footer,
}

/// The layout and language choices of one style.
struct Spec {
    language: Language,
    region: Region,
    sidebar: bool,
    /// Name centered across the page (above the sidebar, when there is one).
    name_full: bool,
    /// Header lines centered (North American) instead of left-aligned.
    center_header: bool,
    /// Summary, experience, education, skills, languages, interests.
    headings: [&'static str; 6],
    upper_headings: bool,
    date_styles: &'static [DateStyle],
    entry_layouts: &'static [EntryLayout],
    europass: bool,
    a4: bool,
    bullet: &'static str,
}

fn spec(style: CorpusStyle, rng: &mut SplitMix64) -> Spec {
    use DateStyle::{Abbreviated, Dot, Iso, Long, Slash};
    use EntryLayout::{Inline, OrgFirst, RightAligned, Stacked};
    match style {
        CorpusStyle::UsChronological => Spec {
            language: Language::En,
            region: Region::Us,
            sidebar: false,
            name_full: true,
            center_header: rng.chance(50),
            headings: [
                "Summary",
                "Experience",
                "Education",
                "Skills",
                "Languages",
                "Interests",
            ],
            upper_headings: rng.chance(50),
            date_styles: &[Abbreviated, Long, Slash, Iso],
            entry_layouts: &[RightAligned, RightAligned, OrgFirst, Inline],
            europass: false,
            a4: false,
            bullet: if rng.chance(50) { "•" } else { "-" },
        },
        CorpusStyle::CanadaEnglish => Spec {
            language: Language::En,
            region: Region::Ca,
            sidebar: false,
            name_full: true,
            center_header: rng.chance(50),
            headings: [
                "Professional Summary",
                "Work Experience",
                "Education",
                "Technical Skills",
                "Languages",
                "Interests",
            ],
            upper_headings: false,
            date_styles: &[Long, Abbreviated, Slash, Iso],
            entry_layouts: &[RightAligned, OrgFirst, Inline, Stacked],
            europass: false,
            a4: false,
            bullet: "-",
        },
        CorpusStyle::CanadaFrench => Spec {
            language: Language::Fr,
            region: Region::CaFr,
            sidebar: false,
            name_full: true,
            center_header: rng.chance(50),
            headings: [
                "Profil professionnel",
                "Expérience professionnelle",
                "Formation",
                "Compétences",
                "Langues",
                "Intérêts",
            ],
            upper_headings: false,
            date_styles: &[Long, Abbreviated, Slash],
            entry_layouts: &[RightAligned, OrgFirst, Inline],
            europass: false,
            a4: false,
            bullet: "•",
        },
        CorpusStyle::EuropassEnglish => Spec {
            language: Language::En,
            region: Region::De,
            sidebar: true,
            name_full: false,
            center_header: false,
            headings: [
                "About me",
                "Work experience",
                "Education & Training",
                "Digital skills",
                "Language skills",
                "Interests",
            ],
            upper_headings: true,
            date_styles: &[Slash, Abbreviated, Long],
            entry_layouts: &[Stacked, OrgFirst],
            europass: true,
            a4: true,
            bullet: "•",
        },
        CorpusStyle::EuropassGerman => Spec {
            language: Language::De,
            region: Region::De,
            sidebar: true,
            name_full: true,
            center_header: true,
            headings: [
                "Über mich",
                "Berufserfahrung",
                "Ausbildung und Weiterbildung",
                "Digitale Kompetenzen",
                "Sprachkenntnisse",
                "Interessen",
            ],
            upper_headings: true,
            date_styles: &[Dot, Long, Abbreviated, Slash],
            entry_layouts: &[Stacked, OrgFirst],
            europass: true,
            a4: true,
            bullet: "•",
        },
        CorpusStyle::EuropassAlbanian => Spec {
            language: Language::Sq,
            region: Region::Sq,
            sidebar: true,
            name_full: false,
            center_header: false,
            headings: [
                "Rreth meje",
                "Përvoja e punës",
                "Arsimi dhe trajnimi",
                "Aftësitë dixhitale",
                "Njohuritë gjuhësore",
                "Interesat",
            ],
            upper_headings: true,
            date_styles: &[Slash, Long, Abbreviated, Dot],
            entry_layouts: &[Stacked, OrgFirst],
            europass: true,
            a4: true,
            bullet: "•",
        },
        CorpusStyle::EuropassFrench => Spec {
            language: Language::Fr,
            region: Region::Fr,
            sidebar: true,
            name_full: true,
            center_header: true,
            headings: [
                "À propos de moi",
                "Expérience professionnelle",
                "Formation et diplômes",
                "Compétences numériques",
                "Compétences linguistiques",
                "Centres d'intérêt",
            ],
            upper_headings: true,
            date_styles: &[Slash, Long, Abbreviated],
            entry_layouts: &[Stacked, OrgFirst],
            europass: true,
            a4: true,
            bullet: "•",
        },
        _ => Spec {
            language: Language::De,
            region: Region::De,
            sidebar: false,
            name_full: false,
            center_header: false,
            headings: [
                "Kurzprofil",
                "Berufserfahrung",
                "Ausbildung",
                "Kenntnisse",
                "Sprachen",
                "Hobbys",
            ],
            upper_headings: false,
            date_styles: &[Dot, Long, Abbreviated, Slash],
            entry_layouts: &[Stacked, OrgFirst, Inline],
            europass: false,
            a4: true,
            bullet: "•",
        },
    }
}

/// Typography of one document, in centipoints.
struct Typo {
    body: i32,
    side: i32,
    heading: i32,
    name: i32,
    mode: HeadingMode,
}

fn typography(spec: &Spec, rng: &mut SplitMix64) -> Typo {
    let body = rng.range(18, 22) * 50;
    let mode = if spec.upper_headings && rng.chance(60) {
        HeadingMode::Caps
    } else {
        match rng.below(3) {
            0 => HeadingMode::Bold,
            1 => HeadingMode::Caps,
            _ => HeadingMode::Plain,
        }
    };
    Typo {
        body,
        side: body.min(1000),
        heading: rng.range(21, 28) * 50,
        name: if spec.sidebar && !spec.name_full {
            rng.range(14, 16) * 100
        } else {
            rng.range(14, 24) * 100
        },
        mode,
    }
}

/// A month and year.
type Month = (i32, u32);

/// One period of a timeline: the end is `None` when ongoing.
struct Span {
    start: Month,
    end: Option<Month>,
}

/// Education and jobs on one timeline, most recent first, ending by 2025.
fn timeline(rng: &mut SplitMix64, jobs: usize, studies: usize) -> (Vec<Span>, Vec<Span>) {
    let study_years: Vec<i32> = (0..studies).map(|_| rng.range(2, 4)).collect();
    let job_years: Vec<i32> = (0..jobs).map(|_| rng.range(1, 3)).collect();
    let total: i32 = study_years.iter().chain(job_years.iter()).sum();
    let mut year = (2025 - total - rng.range(0, 3)).max(1990);
    let mut education = Vec::new();
    for years in study_years {
        education.push(Span {
            start: (year, 9),
            end: Some((year + years, 6)),
        });
        year += years;
    }
    let mut spans = Vec::new();
    for (index, years) in job_years.iter().enumerate() {
        let start = (year, 7);
        let end = (year + years, u32::try_from(rng.range(1, 12)).unwrap_or(1));
        let ongoing = index + 1 == jobs && rng.chance(50);
        spans.push(Span {
            start,
            end: if ongoing { None } else { Some(end) },
        });
        year = end.0;
    }
    spans.reverse();
    education.reverse();
    (education, spans)
}

/// Shared state of one document's generation.
struct Ctx {
    spec: Spec,
    typo: Typo,
    page: Page,
    date_style: DateStyle,
    layout: EntryLayout,
}

impl Ctx {
    fn data(&self) -> &'static super::data::LangData {
        lang_data(self.spec.language)
    }

    fn pools(&self) -> &'static super::pools::Pools {
        lang_pools(self.spec.language)
    }
}

fn shuffle<T>(rng: &mut SplitMix64, values: &mut [T]) {
    for index in (1..values.len()).rev() {
        let other = rng.below(index + 1);
        values.swap(index, other);
    }
}

fn month_text(ctx: &Ctx, month: Month) -> String {
    let (year, month) = month;
    let index = usize::try_from(month.saturating_sub(1))
        .unwrap_or(0)
        .min(11);
    let data = ctx.data();
    match ctx.date_style {
        DateStyle::Abbreviated => format!("{} {year}", data.months_short[index]),
        DateStyle::Long => format!("{} {year}", data.months_long[index]),
        DateStyle::Slash => format!("{month:02}/{year}"),
        DateStyle::Dot => format!("{month:02}.{year}"),
        DateStyle::Iso => format!("{year}-{month:02}"),
    }
}

fn range_text(ctx: &Ctx, rng: &mut SplitMix64, span: &Span, years_only: bool) -> String {
    let start = if years_only {
        span.start.0.to_string()
    } else {
        month_text(ctx, span.start)
    };
    let end = match span.end {
        Some(end) if years_only => end.0.to_string(),
        Some(end) => month_text(ctx, end),
        None => {
            let marker = *rng.pick(ctx.data().ongoing);
            if rng.chance(10) {
                marker.to_uppercase()
            } else {
                marker.to_owned()
            }
        }
    };
    let roll = rng.below(100);
    let numeric_iso = ctx.date_style == DateStyle::Iso && !years_only;
    let separator = if roll < 40 {
        " – ".to_owned()
    } else if roll < 55 {
        " - ".to_owned()
    } else if roll < 60 {
        " — ".to_owned()
    } else if roll < 65 {
        "–".to_owned()
    } else if roll < 70 && !numeric_iso {
        "-".to_owned()
    } else {
        let word = *rng.pick(ctx.pools().range_words);
        let marker_start = end.to_lowercase();
        if marker_start.starts_with(word) {
            " – ".to_owned()
        } else {
            format!(" {word} ")
        }
    };
    format!("{start}{separator}{end}")
}

/// The short place, the full address and the postal city of a region.
pub(super) struct Place {
    pub(super) short: String,
    pub(super) address: String,
}

pub(super) fn place(rng: &mut SplitMix64, region: Region) -> Place {
    let number = rng.range(10, 480);
    match region {
        Region::Us => {
            let (city, state, zip) = *rng.pick(US_CITIES);
            Place {
                short: format!("{city}, {state}"),
                address: format!("{number} {}, {city}, {state} {zip}", rng.pick(EN_STREETS)),
            }
        }
        Region::Ca | Region::CaFr => {
            let (city, province, postal) = *rng.pick(CA_CITIES);
            let street = if region == Region::Ca {
                *rng.pick(EN_STREETS)
            } else {
                "rue Exemple"
            };
            Place {
                short: format!("{city}, {province}"),
                address: format!("{number} {street}, {city}, {province} {postal}"),
            }
        }
        Region::De => {
            let (city, zip) = *rng.pick(DE_CITIES);
            Place {
                short: city.to_owned(),
                address: format!("{} {}, {zip} {city}", rng.pick(DE_STREETS), number % 90 + 1),
            }
        }
        Region::Sq => {
            let (city, zip) = *rng.pick(SQ_CITIES);
            Place {
                short: city.to_owned(),
                address: format!("{} {}, {zip} {city}", rng.pick(SQ_STREETS), number % 90 + 1),
            }
        }
        Region::Fr => {
            let (city, zip) = *rng.pick(FR_CITIES);
            Place {
                short: city.to_owned(),
                address: format!("{} {}, {zip} {city}", number % 90 + 1, rng.pick(FR_STREETS)),
            }
        }
    }
}

/// A fictional phone number: NANP `555-01xx`, the reserved fictional ranges of Germany (`030 23125`)
/// and France (`01 99 00`), and the obviously invalid `000 00xx` for Albania.
pub(super) fn phone(rng: &mut SplitMix64, region: Region, europass: bool) -> String {
    let nn = rng.range(0, 99);
    match region {
        Region::Us | Region::Ca | Region::CaFr => {
            let areas: &[i32] = if region == Region::Us {
                &[202, 212, 312, 415, 206, 404]
            } else {
                &[416, 514, 604, 613, 403]
            };
            let area = *rng.pick(areas);
            if europass {
                return format!("(+1) {area}55501{nn:02}");
            }
            match rng.below(4) {
                0 => format!("({area}) 555-01{nn:02}"),
                1 => format!("{area}-555-01{nn:02}"),
                2 => format!("+1 {area} 555 01{nn:02}"),
                _ => format!("{area}.555.01{nn:02}"),
            }
        }
        Region::De => {
            let line = rng.range(0, 999);
            if europass {
                format!("(+49) 30 23125 {line:03}")
            } else {
                format!("+49 30 23125 {line:03}")
            }
        }
        Region::Sq => {
            if europass {
                format!("(+355) 69 000 00{nn:02}")
            } else {
                format!("+355 69 000 00{nn:02}")
            }
        }
        Region::Fr => {
            let (a, b) = (rng.range(10, 99), rng.range(10, 99));
            if europass {
                format!("(+33) 1 99 00 {a} {b}")
            } else {
                format!("+33 1 99 00 {a} {b}")
            }
        }
    }
}

fn heading(ctx: &Ctx, rng: &mut SplitMix64, kind: SectionKind, slot: Slot) -> Item {
    let index = SectionKind::ALL
        .iter()
        .position(|candidate| *candidate == kind)
        .unwrap_or(0);
    let core = &ctx.spec.headings;
    let extra = &ctx.pools().std_extra;
    let standard = match kind {
        SectionKind::Summary => core[0],
        SectionKind::Experience => core[1],
        SectionKind::Education => core[2],
        SectionKind::Skills => core[3],
        SectionKind::Languages => core[4],
        SectionKind::Interests => core[5],
        SectionKind::Projects => extra[0],
        SectionKind::Certifications => extra[1],
        SectionKind::Volunteering => extra[2],
        SectionKind::Publications => extra[3],
        SectionKind::References => extra[4],
        SectionKind::Contact => extra[5],
    };
    let chosen = pick_held(rng, &[standard], ctx.pools().held_headings[index]);
    let text = if ctx.typo.mode == HeadingMode::Caps {
        chosen.to_uppercase()
    } else {
        chosen.to_owned()
    };
    let size = if slot == Slot::Side {
        ctx.typo.heading.min(1100)
    } else {
        ctx.typo.heading
    };
    let item = Item::resume(text, Role::SectionHeading, slot)
        .size(size)
        .gap();
    if ctx.typo.mode == HeadingMode::Bold {
        item.bold()
    } else {
        item
    }
}

/// Appends wrapped body lines.
fn body_lines(items: &mut Vec<Item>, text: &str, width: usize, slot: Slot, size: i32) {
    for line in wrap(text, width) {
        items.push(Item::resume(line, Role::Body, slot).size(size));
    }
}

/// The wrap width of main-column text of this size.
fn main_width(ctx: &Ctx) -> usize {
    ctx.page.main_capacity(ctx.typo.body)
}

fn bullets(items: &mut Vec<Item>, ctx: &Ctx, rng: &mut SplitMix64) {
    let count = if rng.chance(10) { 0 } else { rng.range(1, 4) };
    let pool_len = ctx.data().bullets.len() + ctx.pools().extra_bullets.len();
    for _ in 0..count {
        let pick = |rng: &mut SplitMix64| -> String {
            let index = rng.below(pool_len);
            let data = ctx.data();
            if index < data.bullets.len() {
                data.bullets[index].to_owned()
            } else {
                ctx.pools().extra_bullets[index - data.bullets.len()].to_owned()
            }
        };
        let mut text = pick(rng);
        if rng.chance(25) {
            text = format!("{text}; {}", pick(rng).to_lowercase());
        }
        let width = main_width(ctx).saturating_sub(4).max(20);
        for (position, line) in wrap(&text, width).into_iter().enumerate() {
            let line = if position == 0 {
                format!("{} {line}", ctx.spec.bullet)
            } else {
                format!("  {line}")
            };
            items.push(Item::resume(line, Role::Bullet, Slot::Main).size(ctx.typo.body));
        }
    }
}

/// One job or study entry in the layout of the document.
fn entry(
    items: &mut Vec<Item>,
    ctx: &Ctx,
    rng: &mut SplitMix64,
    organization: &str,
    title: &str,
    dates: &str,
    city: &str,
) {
    let body = ctx.typo.body;
    let at = ctx.pools().at_word;
    match ctx.layout {
        EntryLayout::RightAligned => {
            items.push(
                Item::resume(organization, Role::Organization, Slot::Main)
                    .size(body)
                    .bold()
                    .gap(),
            );
            items.push(Item::resume(city, Role::Location, Slot::RightOfPrev).size(body));
            items.push(
                Item::resume(title, Role::Title, Slot::Main)
                    .size(body)
                    .italic(),
            );
            items.push(Item::resume(dates, Role::DateRange, Slot::RightOfPrev).size(body));
        }
        EntryLayout::Stacked => {
            items.push(
                Item::resume(dates, Role::DateRange, Slot::Main)
                    .size(body)
                    .gap(),
            );
            items.push(
                Item::resume(title, Role::Title, Slot::Main)
                    .size(body)
                    .bold(),
            );
            items.push(
                Item::resume(
                    format!("{organization}, {city}"),
                    Role::Organization,
                    Slot::Main,
                )
                .size(body),
            );
        }
        EntryLayout::OrgFirst => {
            if rng.chance(50) {
                items.push(
                    Item::resume(
                        format!("{organization}, {city}"),
                        Role::Organization,
                        Slot::Main,
                    )
                    .size(body)
                    .bold()
                    .gap(),
                );
                items.push(
                    Item::resume(title, Role::Title, Slot::Main)
                        .size(body)
                        .italic(),
                );
                items.push(Item::resume(dates, Role::DateRange, Slot::Main).size(body));
            } else {
                items.push(
                    Item::resume(organization, Role::Organization, Slot::Main)
                        .size(body)
                        .bold()
                        .gap(),
                );
                items.push(
                    Item::resume(title, Role::Title, Slot::Main)
                        .size(body)
                        .italic(),
                );
                items.push(Item::resume(dates, Role::DateRange, Slot::Main).size(body));
                items.push(Item::resume(city, Role::Location, Slot::Main).size(body));
            }
        }
        EntryLayout::Inline => {
            if rng.chance(50) {
                items.push(
                    Item::resume(
                        format!("{title} {at} {organization}, {city}"),
                        Role::Title,
                        Slot::Main,
                    )
                    .size(body)
                    .bold()
                    .gap(),
                );
                items.push(Item::resume(dates, Role::DateRange, Slot::Main).size(body));
            } else {
                items.push(
                    Item::resume(format!("{title} | {dates}"), Role::Title, Slot::Main)
                        .size(body)
                        .bold()
                        .gap(),
                );
                items.push(
                    Item::resume(
                        format!("{organization}, {city}"),
                        Role::Organization,
                        Slot::Main,
                    )
                    .size(body),
                );
            }
        }
    }
}

fn experience(items: &mut Vec<Item>, ctx: &Ctx, rng: &mut SplitMix64, jobs: &[Span]) {
    items.push(heading(ctx, rng, SectionKind::Experience, Slot::Main));
    let data = ctx.data();
    for span in jobs {
        let title = pick_held(rng, data.titles, ctx.pools().held_titles).to_owned();
        let employer = company(rng, ctx.spec.language);
        let city = place(rng, ctx.spec.region).short;
        let dates = range_text(ctx, rng, span, false);
        entry(items, ctx, rng, &employer, &title, &dates, &city);
        bullets(items, ctx, rng);
    }
}

fn education(items: &mut Vec<Item>, ctx: &Ctx, rng: &mut SplitMix64, studies: &[Span]) {
    items.push(heading(ctx, rng, SectionKind::Education, Slot::Main));
    let data = ctx.data();
    for span in studies {
        let degree = pick_held(rng, data.degrees, ctx.pools().held_degrees).to_owned();
        let school = (*rng.pick(data.schools)).to_owned();
        let city = place(rng, ctx.spec.region).short;
        let years_only = !ctx.spec.europass && rng.chance(60);
        let dates = range_text(ctx, rng, span, years_only);
        entry(items, ctx, rng, &school, &degree, &dates, &city);
    }
}

fn skill_lines(ctx: &Ctx, rng: &mut SplitMix64) -> Vec<String> {
    let pool = ctx.data().skills;
    let count = usize::try_from(rng.range(0, 4)).unwrap_or(0);
    let labels = ctx.pools().skill_labels;
    let start = rng.below(pool.len());
    let take = |from: usize, n: usize| -> String {
        (0..n)
            .map(|offset| pool[(from + offset) % pool.len()])
            .collect::<Vec<_>>()
            .join(", ")
    };
    if count > 0 && rng.chance(25) {
        return vec![take(start, 5)];
    }
    (0..count)
        .map(|index| {
            format!(
                "{}: {}",
                labels[index % 4],
                take(start + index * 3, 3 + index % 2)
            )
        })
        .collect()
}

fn year_range(rng: &mut SplitMix64) -> String {
    let start = rng.range(2012, 2022);
    format!("{start} – {}", start + rng.range(0, 3))
}

/// The remaining main-column sections.
fn extra_section(items: &mut Vec<Item>, ctx: &Ctx, rng: &mut SplitMix64, kind: SectionKind) {
    let body = ctx.typo.body;
    let (data, pools) = (ctx.data(), ctx.pools());
    items.push(heading(ctx, rng, kind, Slot::Main));
    match kind {
        SectionKind::Projects => {
            for _ in 0..rng.range(1, 3) {
                items.push(
                    Item::resume(*rng.pick(pools.project_names), Role::Title, Slot::Main)
                        .size(body)
                        .bold()
                        .gap(),
                );
                if rng.chance(60) {
                    items.push(
                        Item::resume(year_range(rng), Role::DateRange, Slot::Main).size(body),
                    );
                }
                let width = main_width(ctx);
                body_lines(
                    items,
                    data.summaries[rng.below(data.summaries.len())],
                    width,
                    Slot::Main,
                    body,
                );
            }
        }
        SectionKind::Certifications => {
            for _ in 0..rng.range(1, 3) {
                items.push(
                    Item::resume(*rng.pick(pools.cert_names), Role::Title, Slot::Main)
                        .size(body)
                        .gap(),
                );
                let year = rng.range(2012, 2024);
                items.push(
                    Item::resume(
                        format!("{}, {year}", rng.pick(data.schools)),
                        Role::Organization,
                        Slot::Main,
                    )
                    .size(body),
                );
            }
        }
        SectionKind::Volunteering => {
            for _ in 0..rng.range(1, 2) {
                items.push(
                    Item::resume(
                        *rng.pick(pools.volunteer_orgs),
                        Role::Organization,
                        Slot::Main,
                    )
                    .size(body)
                    .bold()
                    .gap(),
                );
                items.push(
                    Item::resume(*rng.pick(pools.volunteer_roles), Role::Title, Slot::Main)
                        .size(body)
                        .italic(),
                );
                items.push(Item::resume(year_range(rng), Role::DateRange, Slot::Main).size(body));
            }
        }
        SectionKind::Publications => {
            for _ in 0..rng.range(1, 3) {
                items.push(
                    Item::resume(*rng.pick(pools.pub_titles), Role::Title, Slot::Main)
                        .size(body)
                        .gap(),
                );
                let year = rng.range(2012, 2024);
                items.push(
                    Item::resume(
                        format!("{}, {year}", rng.pick(data.schools)),
                        Role::Body,
                        Slot::Main,
                    )
                    .size(body),
                );
            }
        }
        _ => {
            items.push(Item::resume(data.references_note, Role::Body, Slot::Main).size(body));
        }
    }
}

/// The contact lines of a person, in a random order, with the address split on request.
fn contact_items(
    ctx: &Ctx,
    rng: &mut SplitMix64,
    person: &Person,
    slot: Slot,
    size: i32,
    allow_wrap: bool,
) -> Vec<Item> {
    let side = slot == Slot::Side;
    let left_aligned = matches!(slot, Slot::Main | Slot::Side);
    let mut parts: Vec<(&str, String)> = vec![("email", person.email.clone())];
    if rng.chance(85) {
        parts.push(("address", place(rng, ctx.spec.region).address));
    }
    if rng.chance(90) {
        parts.push(("phone", phone(rng, ctx.spec.region, ctx.spec.europass)));
    }
    if rng.chance(60) {
        parts.push((
            "url",
            if side {
                person.url_short.clone()
            } else {
                person.url_long.clone()
            },
        ));
    }
    shuffle(rng, &mut parts);
    let mut items = Vec::new();
    for (kind, text) in parts {
        match kind {
            "address" => {
                // A sidebar line must stay inside the sidebar: split what would be too wide.
                let too_wide =
                    side && i32::try_from(text.chars().count()).unwrap_or(0) * size / 2 > 17_000;
                let wrap_it = allow_wrap && left_aligned && (too_wide || rng.chance(45));
                match text.split_once(", ") {
                    Some((street, rest)) if wrap_it => {
                        items.push(
                            Item::resume(format!("{street},"), Role::Contact, slot).size(size),
                        );
                        items.push(
                            Item::resume(rest, Role::Contact, slot)
                                .size(size)
                                .continues(" "),
                        );
                    }
                    Some((street, rest)) if side && rng.chance(50) => {
                        items.push(Item::resume(street, Role::Contact, slot).size(size));
                        items.push(Item::resume(rest, Role::Contact, slot).size(size));
                    }
                    _ => items.push(Item::resume(text, Role::Contact, slot).size(size)),
                }
            }
            "url" if allow_wrap && slot == Slot::Main && rng.chance(40) => {
                let tail_len = usize::try_from(rng.range(3, 8)).unwrap_or(4);
                let cut = text.len() - tail_len;
                items.push(
                    Item::resume(&text[..cut], Role::Contact, slot)
                        .size(size)
                        .full_width(),
                );
                items.push(
                    Item::resume(&text[cut..], Role::Contact, slot)
                        .size(size)
                        .continues(""),
                );
            }
            _ => items.push(Item::resume(text, Role::Contact, slot).size(size)),
        }
    }
    items
}

/// Builds the page and the items of one resume.
pub(super) fn build(style: CorpusStyle, rng: &mut SplitMix64) -> (Page, Vec<Item>) {
    let spec = spec(style, rng);
    let typo = typography(&spec, rng);
    let page = match (spec.sidebar, spec.a4) {
        (true, _) => Page::sidebar(59500, 84200),
        (false, true) => Page::single_column(59500, 84200),
        (false, false) => Page::single_column(61200, 79200),
    };
    let date_style = *rng.pick(spec.date_styles);
    let layout = *rng.pick(spec.entry_layouts);
    let ctx = Ctx {
        spec,
        typo,
        page,
        date_style,
        layout,
    };
    let (data, pools) = (ctx.data(), ctx.pools());
    let person = Person::new(rng, data);
    let job_count = usize::try_from(rng.range(1, 5)).unwrap_or(2);
    let study_count = usize::try_from(rng.range(1, 3)).unwrap_or(1);
    let (studies, jobs) = timeline(rng, job_count, study_count);
    let sidebar = ctx.spec.sidebar;
    let body = ctx.typo.body;

    // Header: an optional document title, the name and the contact lines.
    let mut top: Vec<Item> = Vec::new();
    let mut side: Vec<Item> = Vec::new();
    let mut footer: Vec<Item> = Vec::new();
    let header_slot = if sidebar {
        Slot::Side
    } else if ctx.spec.center_header {
        Slot::Full(Align::Center)
    } else {
        Slot::Main
    };
    if rng.chance(25) {
        let slot = if ctx.spec.center_header || sidebar {
            Slot::Full(Align::Center)
        } else {
            Slot::Main
        };
        top.push(
            Item::resume(*rng.pick(pools.doc_titles), Role::Other, slot)
                .size(ctx.typo.heading + 200)
                .bold(),
        );
    }
    let name_slot = if ctx.spec.name_full && sidebar {
        Slot::Full(Align::Center)
    } else if ctx.spec.name_full {
        header_slot
    } else if sidebar {
        Slot::Side
    } else {
        Slot::Main
    };
    let name = Item::resume(person.full_name(), Role::Name, name_slot)
        .size(ctx.typo.name)
        .bold();
    let contact_size = if sidebar { ctx.typo.side } else { body };
    let name_in_top = name_slot != Slot::Side;
    let place_choice = {
        let roll = rng.below(100);
        if sidebar {
            if roll < 85 || name_in_top {
                ContactPlace::AfterName
            } else {
                ContactPlace::BeforeName
            }
        } else if roll < 55 {
            ContactPlace::AfterName
        } else if roll < 75 {
            ContactPlace::Band
        } else if roll < 85 {
            ContactPlace::BeforeName
        } else {
            ContactPlace::Footer
        }
    };
    let contact_slot = if sidebar { Slot::Side } else { header_slot };
    let mut contacts = contact_items(&ctx, rng, &person, contact_slot, contact_size, true);
    if place_choice == ContactPlace::Band {
        // One line: `address | phone | email`.
        let mut band: Vec<String> = Vec::new();
        for item in &contacts {
            match (item.continues, band.last_mut()) {
                (Some(separator), Some(last)) => {
                    last.push_str(separator);
                    last.push_str(&item.text);
                }
                _ => band.push(item.text.clone()),
            }
        }
        // Two parts per band line keep the line inside the page.
        contacts = band
            .chunks(2)
            .map(|chunk| {
                Item::resume(chunk.join(" | "), Role::Contact, contact_slot).size(contact_size)
            })
            .collect();
    }
    match place_choice {
        ContactPlace::BeforeName => {
            if name_in_top {
                // The contact lines come first, then the name.
                top.extend(contacts);
                top.push(name);
            } else {
                side.extend(contacts);
                side.push(name);
            }
        }
        ContactPlace::Footer => {
            if name_in_top {
                top.push(name);
            } else {
                side.push(name);
            }
            footer.extend(contacts);
            if let Some(first) = footer.first_mut() {
                first.gap = true;
            }
        }
        _ => {
            let target = if name_in_top { &mut top } else { &mut side };
            target.push(name);
            if sidebar {
                side.extend(contacts);
            } else {
                top.extend(contacts);
            }
        }
    }

    // Sections.
    let mut main: Vec<Item> = Vec::new();
    if sidebar {
        let mut blocks: Vec<SectionKind> = vec![SectionKind::Skills, SectionKind::Languages];
        if rng.chance(40) {
            blocks.push(SectionKind::Interests);
        }
        if rng.chance(15) {
            blocks.remove(0);
        }
        shuffle(rng, &mut blocks);
        for kind in blocks {
            let lines: Vec<String> = match kind {
                SectionKind::Skills => skill_lines(&ctx, rng),
                SectionKind::Languages => {
                    let source = if ctx.spec.europass {
                        data.europass_language_lines
                    } else {
                        data.language_lines
                    };
                    source.iter().map(|line| (*line).to_owned()).collect()
                }
                _ => vec![data.interests.join(", ")],
            };
            if lines.is_empty() {
                continue;
            }
            side.push(heading(&ctx, rng, kind, Slot::Side));
            for line in lines {
                body_lines(&mut side, &line, 30, Slot::Side, ctx.typo.side);
            }
        }
    }

    // Main-column sections in a random order: experience early, references late.
    let mut sections: Vec<SectionKind> = vec![SectionKind::Experience, SectionKind::Education];
    let optional: [(SectionKind, usize); 5] = [
        (SectionKind::Projects, 35),
        (SectionKind::Certifications, 35),
        (SectionKind::Volunteering, 25),
        (SectionKind::Publications, 20),
        (SectionKind::Interests, 40),
    ];
    if !sidebar {
        sections.push(SectionKind::Skills);
        if rng.chance(55) {
            sections.push(SectionKind::Languages);
        }
    }
    for (kind, percent) in optional {
        let allowed = !(sidebar && kind == SectionKind::Interests);
        if allowed && rng.chance(percent) {
            sections.push(kind);
        }
    }
    shuffle(rng, &mut sections);
    if rng.chance(60)
        && let Some(position) = sections.iter().position(|k| *k == SectionKind::Experience)
    {
        let experience = sections.remove(position);
        sections.insert(0, experience);
    }
    if rng.chance(75) {
        sections.insert(0, SectionKind::Summary);
    }
    if rng.chance(30) {
        sections.push(SectionKind::References);
    }
    for kind in sections {
        match kind {
            SectionKind::Summary => {
                main.push(heading(&ctx, rng, kind, Slot::Main));
                let count = usize::try_from(rng.range(1, 3)).unwrap_or(1);
                let text = (0..count)
                    .map(|_| data.summaries[rng.below(data.summaries.len())])
                    .collect::<Vec<_>>()
                    .join(" ");
                body_lines(&mut main, &text, main_width(&ctx), Slot::Main, body);
            }
            SectionKind::Experience => experience(&mut main, &ctx, rng, &jobs),
            SectionKind::Education => education(&mut main, &ctx, rng, &studies),
            SectionKind::Skills => {
                let lines = skill_lines(&ctx, rng);
                if !lines.is_empty() {
                    main.push(heading(&ctx, rng, kind, Slot::Main));
                    for line in lines {
                        body_lines(&mut main, &line, main_width(&ctx), Slot::Main, body);
                    }
                }
            }
            SectionKind::Languages => {
                main.push(heading(&ctx, rng, kind, Slot::Main));
                for line in data.language_lines {
                    main.push(Item::resume(*line, Role::Body, Slot::Main).size(body));
                }
            }
            SectionKind::Interests => {
                main.push(heading(&ctx, rng, kind, Slot::Main));
                body_lines(
                    &mut main,
                    &data.interests.join(", "),
                    main_width(&ctx),
                    Slot::Main,
                    body,
                );
            }
            other => extra_section(&mut main, &ctx, rng, other),
        }
    }

    // Stray single words in the main flow, before items that start a block.
    for _ in 0..rng.range(0, 2) {
        let safe: Vec<usize> = (1..main.len())
            .filter(|i| main[*i].slot == Slot::Main && main[*i].continues.is_none())
            .collect();
        if safe.is_empty() {
            break;
        }
        let at = safe[rng.below(safe.len())];
        main.insert(
            at,
            Item::resume(*rng.pick(STRAY_WORDS), Role::Other, Slot::Main).size(body),
        );
    }

    let mut items = top;
    items.extend(side);
    items.extend(main);
    items.extend(footer);
    (ctx.page, items)
}
