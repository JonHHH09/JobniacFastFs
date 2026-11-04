//! The per-language lexicons: section headings, cover-letter markers, company suffixes, degree
//! keywords, job-title keywords and month names. All tables live here as data.
//!
//! Matching is case- and accent-insensitive: both the table entry and the text go through
//! [`fold`], which is `jobniac_text::search::normalize` (lower-case, NFKD, marks dropped) plus a few
//! letters that decomposition leaves alone (`ß`, `œ`, `æ`) and one apostrophe form.

use std::sync::LazyLock;

use jobniac_text::search::normalize;

/// A language of the lexicons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Language {
    /// English (US, Canada, Europass).
    En,
    /// French (Canada, Europass).
    Fr,
    /// German.
    De,
    /// Albanian.
    Sq,
}

impl Language {
    /// Every language, in table order.
    pub const ALL: [Language; 4] = [Language::En, Language::Fr, Language::De, Language::Sq];
}

/// The kind of a resume section, as a heading names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SectionKind {
    /// Summary, profile, about me.
    Summary,
    /// Work experience.
    Experience,
    /// Education and training.
    Education,
    /// Skills, including digital skills.
    Skills,
    /// Languages, language skills, mother tongue.
    Languages,
    /// Projects.
    Projects,
    /// Certifications and courses.
    Certifications,
    /// Volunteering.
    Volunteering,
    /// Publications.
    Publications,
    /// References.
    References,
    /// Interests and hobbies.
    Interests,
    /// Contact and personal details.
    Contact,
}

impl SectionKind {
    /// Every kind, in feature order.
    pub const ALL: [SectionKind; 12] = [
        SectionKind::Summary,
        SectionKind::Experience,
        SectionKind::Education,
        SectionKind::Skills,
        SectionKind::Languages,
        SectionKind::Projects,
        SectionKind::Certifications,
        SectionKind::Volunteering,
        SectionKind::Publications,
        SectionKind::References,
        SectionKind::Interests,
        SectionKind::Contact,
    ];

    /// The stable lower-case name of the kind (used in feature names).
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            SectionKind::Summary => "summary",
            SectionKind::Experience => "experience",
            SectionKind::Education => "education",
            SectionKind::Skills => "skills",
            SectionKind::Languages => "languages",
            SectionKind::Projects => "projects",
            SectionKind::Certifications => "certifications",
            SectionKind::Volunteering => "volunteering",
            SectionKind::Publications => "publications",
            SectionKind::References => "references",
            SectionKind::Interests => "interests",
            SectionKind::Contact => "contact",
        }
    }
}

use Language::{De, En, Fr, Sq};
use SectionKind::{
    Certifications, Contact, Education, Experience, Interests, Languages, Projects, Publications,
    References, Skills, Summary, Volunteering,
};

/// Resume section headings, matched against the whole folded line (without a trailing colon).
pub const HEADINGS: &[(SectionKind, Language, &str)] = &[
    (Summary, En, "Summary"),
    (Summary, En, "Professional Summary"),
    (Summary, En, "Profile"),
    (Summary, En, "Professional Profile"),
    (Summary, En, "Personal Profile"),
    (Summary, En, "About Me"),
    (Summary, En, "Objective"),
    (Summary, En, "Career Objective"),
    (Summary, En, "Personal Statement"),
    (Summary, Fr, "Sommaire"),
    (Summary, Fr, "Profil"),
    (Summary, Fr, "Profil professionnel"),
    (Summary, Fr, "À propos de moi"),
    (Summary, Fr, "Objectif professionnel"),
    (Summary, De, "Zusammenfassung"),
    (Summary, De, "Profil"),
    (Summary, De, "Kurzprofil"),
    (Summary, De, "Berufsprofil"),
    (Summary, De, "Über mich"),
    (Summary, Sq, "Përmbledhje"),
    (Summary, Sq, "Profili"),
    (Summary, Sq, "Profili profesional"),
    (Summary, Sq, "Rreth meje"),
    (Summary, Sq, "Objektivi"),
    (Experience, En, "Experience"),
    (Experience, En, "Work Experience"),
    (Experience, En, "Professional Experience"),
    (Experience, En, "Employment"),
    (Experience, En, "Employment History"),
    (Experience, En, "Work History"),
    (Experience, Fr, "Expérience"),
    (Experience, Fr, "Expérience professionnelle"),
    (Experience, Fr, "Expériences professionnelles"),
    (Experience, Fr, "Parcours professionnel"),
    (Experience, Fr, "Emploi"),
    (Experience, De, "Berufserfahrung"),
    (Experience, De, "Beruflicher Werdegang"),
    (Experience, De, "Arbeitserfahrung"),
    (Experience, De, "Erfahrung"),
    (Experience, De, "Werdegang"),
    (Experience, Sq, "Përvoja e punës"),
    (Experience, Sq, "Përvoja profesionale"),
    (Experience, Sq, "Përvoja"),
    (Experience, Sq, "Eksperienca e punës"),
    (Experience, Sq, "Eksperienca"),
    (Education, En, "Education"),
    (Education, En, "Education & Training"),
    (Education, En, "Education and Training"),
    (Education, En, "Training"),
    (Education, Fr, "Formation"),
    (Education, Fr, "Éducation"),
    (Education, Fr, "Études"),
    (Education, Fr, "Formation et diplômes"),
    (Education, Fr, "Scolarité"),
    (Education, De, "Ausbildung"),
    (Education, De, "Bildung"),
    (Education, De, "Schulbildung"),
    (Education, De, "Studium"),
    (Education, De, "Akademischer Werdegang"),
    (Education, De, "Ausbildung und Weiterbildung"),
    (Education, Sq, "Arsimi"),
    (Education, Sq, "Arsimimi"),
    (Education, Sq, "Edukimi"),
    (Education, Sq, "Edukimi dhe trajnimi"),
    (Education, Sq, "Arsimi dhe trajnimi"),
    (Education, Sq, "Formimi"),
    (Skills, En, "Skills"),
    (Skills, En, "Technical Skills"),
    (Skills, En, "Key Skills"),
    (Skills, En, "Core Competencies"),
    (Skills, En, "Digital Skills"),
    (Skills, En, "Personal Skills"),
    (Skills, En, "Technologies"),
    (Skills, En, "Computer skills"),
    (Skills, Fr, "Compétences informatiques"),
    (Skills, En, "Technology Stack"),
    (Skills, Fr, "Compétences"),
    (Skills, Fr, "Compétences techniques"),
    (Skills, Fr, "Compétences numériques"),
    (Skills, Fr, "Aptitudes"),
    (Skills, De, "Kenntnisse"),
    (Skills, De, "Fähigkeiten"),
    (Skills, De, "Kompetenzen"),
    (Skills, De, "IT-Kenntnisse"),
    (Skills, De, "EDV-Kenntnisse"),
    (Skills, De, "Digitale Kompetenzen"),
    (Skills, De, "Technische Fähigkeiten"),
    (Skills, Sq, "Aftësitë"),
    (Skills, Sq, "Aftësi"),
    (Skills, Sq, "Aftësitë teknike"),
    (Skills, Sq, "Aftësitë dixhitale"),
    (Skills, Sq, "Kompetencat"),
    (Languages, En, "Languages"),
    (Languages, En, "Language Skills"),
    (Languages, En, "Foreign Languages"),
    (Languages, En, "Mother tongue(s)"),
    (Languages, En, "Mother tongue"),
    (Languages, En, "Other language(s)"),
    (Languages, En, "Other languages"),
    (Languages, Fr, "Autres langues"),
    (Languages, De, "Weitere Sprachen"),
    (Languages, Sq, "Gjuhë të tjera"),
    (Languages, Fr, "Langues"),
    (Languages, Fr, "Compétences linguistiques"),
    (Languages, Fr, "Langue maternelle"),
    (Languages, De, "Sprachen"),
    (Languages, De, "Sprachkenntnisse"),
    (Languages, De, "Muttersprache"),
    (Languages, Sq, "Gjuhët"),
    (Languages, Sq, "Njohuritë gjuhësore"),
    (Languages, Sq, "Gjuha amtare"),
    (Languages, Sq, "Gjuhët e huaja"),
    (Projects, En, "Projects"),
    (Projects, En, "Personal Projects"),
    (Projects, En, "Selected Projects"),
    (Projects, Fr, "Projets"),
    (Projects, Fr, "Projets personnels"),
    (Projects, De, "Projekte"),
    (Projects, Sq, "Projektet"),
    (Projects, Sq, "Projekte"),
    (Certifications, En, "Certifications"),
    (Certifications, En, "Certificates"),
    (Certifications, En, "Licenses & Certifications"),
    (Certifications, En, "Courses"),
    (Certifications, En, "Driving licence"),
    (Certifications, En, "Driving license"),
    (Certifications, Fr, "Permis de conduire"),
    (Certifications, De, "Führerschein"),
    (Certifications, Sq, "Leje drejtimi"),
    (Certifications, Fr, "Certifications"),
    (Certifications, Fr, "Certificats"),
    (Certifications, De, "Zertifikate"),
    (Certifications, De, "Zertifizierungen"),
    (Certifications, De, "Weiterbildungen"),
    (Certifications, Sq, "Certifikimet"),
    (Certifications, Sq, "Certifikata"),
    (Certifications, Sq, "Kurset"),
    (Volunteering, En, "Volunteering"),
    (Volunteering, En, "Volunteer Experience"),
    (Volunteering, En, "Volunteer Work"),
    (Volunteering, En, "Community Involvement"),
    (Volunteering, Fr, "Bénévolat"),
    (Volunteering, Fr, "Expérience bénévole"),
    (Volunteering, Fr, "Engagement communautaire"),
    (Volunteering, De, "Ehrenamt"),
    (Volunteering, De, "Ehrenamtliches Engagement"),
    (Volunteering, De, "Freiwilligenarbeit"),
    (Volunteering, Sq, "Vullnetarizmi"),
    (Volunteering, Sq, "Puna vullnetare"),
    (Volunteering, Sq, "Përvoja vullnetare"),
    (Publications, En, "Publications"),
    (Publications, En, "Selected Publications"),
    (Publications, Fr, "Publications"),
    (Publications, De, "Veröffentlichungen"),
    (Publications, De, "Publikationen"),
    (Publications, Sq, "Publikimet"),
    (References, En, "References"),
    (References, Fr, "Références"),
    (References, De, "Referenzen"),
    (References, Sq, "Referencat"),
    (References, Sq, "Referenca"),
    (Interests, En, "Interests"),
    (Interests, En, "Hobbies"),
    (Interests, En, "Hobbies & Interests"),
    (Interests, En, "Hobbies and Interests"),
    (Interests, Fr, "Intérêts"),
    (Interests, Fr, "Loisirs"),
    (Interests, Fr, "Centres d'intérêt"),
    (Interests, De, "Interessen"),
    (Interests, De, "Hobbys"),
    (Interests, De, "Hobbys und Interessen"),
    (Interests, Sq, "Interesat"),
    (Interests, Sq, "Hobi"),
    (Interests, Sq, "Hobitë"),
    (Contact, En, "Contact"),
    (Contact, En, "Contact Information"),
    (Contact, En, "Contact Details"),
    (Contact, En, "Personal Information"),
    (Contact, En, "Personal Details"),
    (Contact, Fr, "Coordonnées"),
    (Contact, Fr, "Contact"),
    (Contact, Fr, "Informations personnelles"),
    (Contact, De, "Kontakt"),
    (Contact, De, "Kontaktdaten"),
    (Contact, De, "Persönliche Daten"),
    (Contact, De, "Persönliche Angaben"),
    (Contact, Sq, "Kontakti"),
    (Contact, Sq, "Kontakt"),
    (Contact, Sq, "Të dhënat personale"),
    (Contact, Sq, "Informacioni personal"),
];

/// Cover-letter salutations; a line that starts with one (on a word boundary) is a salutation.
pub const SALUTATIONS: &[(Language, &str)] = &[
    (En, "Dear"),
    (En, "To whom it may concern"),
    (En, "Hello"),
    (Fr, "Madame, Monsieur"),
    (Fr, "Madame"),
    (Fr, "Monsieur"),
    (Fr, "Bonjour"),
    (Fr, "Chère"),
    (Fr, "Cher"),
    (De, "Sehr geehrte"),
    (De, "Sehr geehrter"),
    (De, "Guten Tag"),
    (De, "Liebe"),
    (De, "Lieber"),
    (De, "Hallo"),
    (Sq, "I nderuar"),
    (Sq, "E nderuar"),
    (Sq, "I/E nderuar"),
    (Sq, "Të nderuar"),
];

/// Cover-letter closings; a line that starts with one is a closing.
pub const CLOSINGS: &[(Language, &str)] = &[
    (En, "Sincerely"),
    (En, "Sincerely yours"),
    (En, "Yours sincerely"),
    (En, "Yours faithfully"),
    (En, "Yours truly"),
    (En, "Kind regards"),
    (En, "Best regards"),
    (En, "Warm regards"),
    (En, "Regards"),
    (En, "Respectfully"),
    (Fr, "Cordialement"),
    (Fr, "Bien cordialement"),
    (Fr, "Veuillez agréer"),
    (Fr, "Je vous prie d'agréer"),
    (Fr, "Salutations distinguées"),
    (De, "Mit freundlichen Grüßen"),
    (De, "Freundliche Grüße"),
    (De, "Viele Grüße"),
    (De, "Mit besten Grüßen"),
    (De, "Hochachtungsvoll"),
    (Sq, "Me respekt"),
    (Sq, "Me nderim"),
    (Sq, "Përshëndetje"),
    (Sq, "Faleminderit"),
];

/// Subject markers; a line that starts with one followed by a colon is a subject.
pub const SUBJECT_MARKERS: &[(Language, &str)] = &[
    (En, "Subject"),
    (En, "Re"),
    (En, "Regarding"),
    (Fr, "Objet"),
    (De, "Betreff"),
    (Sq, "Lënda"),
];

/// Recipient markers; a line that starts with one followed by a colon is a recipient line.
pub const RECIPIENT_MARKERS: &[(Language, &str)] = &[
    (En, "To"),
    (En, "Attn"),
    (Fr, "À"),
    (Fr, "Pour"),
    (De, "An"),
    (Sq, "Për"),
];

/// Company legal-form suffixes. Matched case-sensitively, with dots removed, on the last token of
/// a name segment (a segment ends at a comma, a semicolon, a bar, a parenthesis or a spaced dash):
/// `Example Systems GmbH, Berlin` matches, `Limited experience` and `sa` do not.
pub const COMPANY_SUFFIXES: &[&str] = &[
    "Inc.",
    "inc.",
    "LLC",
    "Ltd",
    "ltd.",
    "Ltée",
    "ltée",
    "Corp.",
    "corp.",
    "Corporation",
    "Co.",
    "Limited",
    "LLP",
    "GmbH",
    "AG",
    "KG",
    "UG",
    "e.V.",
    "SA",
    "S.A.",
    "SAS",
    "SARL",
    "Sh.p.k.",
    "Sh.a.",
];

/// Degree keywords. A single word matches a whole token (case- and accent-insensitive, dots
/// ignored, an `s` or `'s` plural allowed): `Bachelor`, `Bachelors`, `B.Sc.`. An entry with a space
/// is a phrase and matches on word boundaries. Hyphenated words are split, so `Diplom-Informatiker`
/// matches `Diplom`.
pub const DEGREE_WORDS: &[(Language, &str)] = &[
    (En, "Bachelor"),
    (En, "Master"),
    (En, "B.Sc."),
    (En, "M.Sc."),
    (En, "BSc"),
    (En, "MSc"),
    (En, "PhD"),
    (En, "Doctorate"),
    (En, "Associate degree"),
    (Fr, "Baccalauréat"),
    (Fr, "Maîtrise en"),
    (Fr, "Maîtrise ès"),
    (Fr, "Licence en"),
    (Fr, "Licence professionnelle"),
    (Fr, "Doctorat"),
    (Fr, "Diplôme"),
    (De, "Bachelor"),
    (De, "Master"),
    (De, "Diplom"),
    (De, "Abitur"),
    (Sq, "Bachelor"),
    (Sq, "Master"),
    (Sq, "Diplomë"),
    (Sq, "Doktoratë"),
];

/// Degree abbreviations that collide with ordinary words when folded (`DEC` and `dec`), so they
/// match case-sensitively, with dots ignored.
pub const DEGREE_CASED: &[&str] = &["DEC", "AEC", "MBA", "BBA"];

/// Degree abbreviations that are also state codes or words (`MA`, `MS Office`): they match only
/// when written with dots (`M.S.`) or followed by `in`, `of` or `en`.
pub const DEGREE_AMBIGUOUS: &[&str] = &["BA", "MA", "BS", "MS"];

/// Words that end a document title, not a section: `Resume`, `CV`, `Lebenslauf`.
pub const DOCUMENT_TITLES: &[&str] = &[
    "Resume",
    "Résumé",
    "CV",
    "Curriculum Vitae",
    "Lebenslauf",
    "Vita",
];

/// Common job-title keywords. A token matches when it equals the keyword, or has one extra
/// letter (plural or gendered ending); German keywords also match as the end of a compound.
pub const TITLE_WORDS: &[(Language, &str)] = &[
    (En, "Engineer"),
    (En, "Developer"),
    (En, "Manager"),
    (En, "Analyst"),
    (En, "Designer"),
    (En, "Consultant"),
    (En, "Director"),
    (En, "Architect"),
    (En, "Assistant"),
    (En, "Specialist"),
    (En, "Administrator"),
    (En, "Intern"),
    (En, "Technician"),
    (En, "Coordinator"),
    (En, "Scientist"),
    (En, "Teacher"),
    (En, "Accountant"),
    (En, "Leader"),
    (Fr, "Ingénieur"),
    (Fr, "Développeur"),
    (Fr, "Gestionnaire"),
    (Fr, "Analyste"),
    (Fr, "Concepteur"),
    (Fr, "Directeur"),
    (Fr, "Directrice"),
    (Fr, "Responsable"),
    (Fr, "Adjoint"),
    (Fr, "Stagiaire"),
    (Fr, "Technicien"),
    (Fr, "Coordonnateur"),
    (Fr, "Conseiller"),
    (De, "Entwickler"),
    (De, "Ingenieur"),
    (De, "Leiter"),
    (De, "Berater"),
    (De, "Sachbearbeiter"),
    (De, "Praktikant"),
    (De, "Werkstudent"),
    (De, "Projektleiter"),
    (De, "Mitarbeiter"),
    (Sq, "Inxhinier"),
    (Sq, "Zhvillues"),
    (Sq, "Menaxher"),
    (Sq, "Analist"),
    (Sq, "Drejtor"),
    (Sq, "Asistent"),
    (Sq, "Specialist"),
    (Sq, "Praktikant"),
    (Sq, "Konsulent"),
    (Sq, "Mësues"),
];

/// Street words (folded). A token matches when it equals the word; German and Albanian words also
/// match as the end of a compound (`beispielstrasse`).
pub const STREET_WORDS: &[(Language, &str)] = &[
    (En, "street"),
    (En, "st"),
    (En, "avenue"),
    (En, "ave"),
    (En, "road"),
    (En, "rd"),
    (En, "boulevard"),
    (En, "blvd"),
    (En, "drive"),
    (En, "lane"),
    (Fr, "rue"),
    (Fr, "chemin"),
    (De, "strasse"),
    (De, "str"),
    (De, "weg"),
    (De, "platz"),
    (De, "allee"),
    (De, "gasse"),
    (Sq, "rruga"),
    (Sq, "bulevardi"),
];

/// Month names and abbreviations, folded form to month number. Abbreviations may be written with a
/// trailing dot.
pub const MONTHS: &[(Language, &str, u8)] = &[
    (En, "january", 1),
    (En, "february", 2),
    (En, "march", 3),
    (En, "april", 4),
    (En, "may", 5),
    (En, "june", 6),
    (En, "july", 7),
    (En, "august", 8),
    (En, "september", 9),
    (En, "october", 10),
    (En, "november", 11),
    (En, "december", 12),
    (En, "jan", 1),
    (En, "feb", 2),
    (En, "mar", 3),
    (En, "apr", 4),
    (En, "jun", 6),
    (En, "jul", 7),
    (En, "aug", 8),
    (En, "sep", 9),
    (En, "sept", 9),
    (En, "oct", 10),
    (En, "nov", 11),
    (En, "dec", 12),
    (Fr, "janvier", 1),
    (Fr, "fevrier", 2),
    (Fr, "mars", 3),
    (Fr, "avril", 4),
    (Fr, "mai", 5),
    (Fr, "juin", 6),
    (Fr, "juillet", 7),
    (Fr, "aout", 8),
    (Fr, "septembre", 9),
    (Fr, "octobre", 10),
    (Fr, "novembre", 11),
    (Fr, "decembre", 12),
    (Fr, "janv", 1),
    (Fr, "fevr", 2),
    (Fr, "avr", 4),
    (Fr, "juil", 7),
    (Fr, "sept", 9),
    (Fr, "dec", 12),
    (Fr, "fev", 2),
    (Fr, "juill", 7),
    (De, "januar", 1),
    (De, "februar", 2),
    (De, "marz", 3),
    (De, "april", 4),
    (De, "mai", 5),
    (De, "juni", 6),
    (De, "juli", 7),
    (De, "august", 8),
    (De, "september", 9),
    (De, "oktober", 10),
    (De, "november", 11),
    (De, "dezember", 12),
    (De, "mrz", 3),
    (De, "okt", 10),
    (De, "dez", 12),
    (De, "febr", 2),
    (De, "sept", 9),
    (Sq, "janar", 1),
    (Sq, "shkurt", 2),
    (Sq, "mars", 3),
    (Sq, "prill", 4),
    (Sq, "maj", 5),
    (Sq, "qershor", 6),
    (Sq, "korrik", 7),
    (Sq, "gusht", 8),
    (Sq, "shtator", 9),
    (Sq, "tetor", 10),
    (Sq, "nentor", 11),
    (Sq, "dhjetor", 12),
    (Sq, "shk", 2),
    (Sq, "pri", 4),
    (Sq, "qer", 6),
    (Sq, "kor", 7),
    (Sq, "gus", 8),
    (Sq, "sht", 9),
    (Sq, "tet", 10),
    (Sq, "nen", 11),
    (Sq, "dhj", 12),
    (Sq, "gush", 8),
    (Sq, "korr", 7),
];

/// Markers of an ongoing position or study, in folded-text form (accents dropped). A range that
/// ends with one is open-ended.
pub const ONGOING_MARKERS: &[(Language, &str)] = &[
    (En, "present"),
    (En, "current"),
    (En, "currently"),
    (En, "now"),
    (En, "ongoing"),
    (En, "to date"),
    (Fr, "present"),
    (Fr, "aujourd'hui"),
    (Fr, "a ce jour"),
    (Fr, "ce jour"),
    (Fr, "en cours"),
    (Fr, "actuellement"),
    (De, "heute"),
    (De, "derzeit"),
    (De, "aktuell"),
    (De, "jetzt"),
    (De, "bis dato"),
    (Sq, "sot"),
    (Sq, "aktualisht"),
    (Sq, "ne vazhdim"),
    (Sq, "me sot"),
    (Sq, "deri me sot"),
    (Sq, "tani"),
];

/// Range separators written as words, in folded form.
pub const RANGE_WORDS: &[(Language, &str)] = &[
    (En, "to"),
    (En, "until"),
    (Fr, "a"),
    (Fr, "au"),
    (De, "bis"),
    (Sq, "deri"),
];

/// Words that may introduce a range (`from 2019 to 2021`), in folded form.
pub const RANGE_INTRO_WORDS: &[(Language, &str)] = &[
    (En, "from"),
    (En, "since"),
    (Fr, "de"),
    (Fr, "depuis"),
    (De, "von"),
    (De, "seit"),
    (Sq, "nga"),
];

/// Words that introduce an end date alone (`until 2020`, `jusqu'à 2020`), in folded form.
pub const END_INTRO_WORDS: &[(Language, &str)] = &[
    (En, "until"),
    (En, "till"),
    (Fr, "jusqu'a"),
    (Fr, "jusqu'au"),
    (De, "bis"),
    (Sq, "deri"),
];

/// Folds text for lexicon matching: lower-case, accents dropped, `ß` as `ss`, one apostrophe form,
/// single spaces.
#[must_use]
pub fn fold(text: &str) -> String {
    let base = normalize(Some(text))
        .replace(['\u{2019}', '\u{2018}', '\u{02bc}', '`'], "'")
        .replace('ß', "ss")
        .replace('œ', "oe")
        .replace('æ', "ae");
    base.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether the folded `line` starts with the folded `phrase` on a word boundary.
fn starts_with_phrase(line: &str, phrase: &str) -> bool {
    line.strip_prefix(phrase)
        .is_some_and(|rest| !rest.chars().next().is_some_and(char::is_alphanumeric))
}

/// The folded entries of a `(Language, &str)` table.
fn folded_pairs(table: &[(Language, &str)]) -> Vec<String> {
    table.iter().map(|(_, phrase)| fold(phrase)).collect()
}

static FOLDED_HEADINGS: LazyLock<Vec<(SectionKind, String)>> = LazyLock::new(|| {
    HEADINGS
        .iter()
        .map(|(kind, _, phrase)| (*kind, fold(phrase)))
        .collect()
});
static FOLDED_SALUTATIONS: LazyLock<Vec<String>> = LazyLock::new(|| folded_pairs(SALUTATIONS));
static FOLDED_CLOSINGS: LazyLock<Vec<String>> = LazyLock::new(|| folded_pairs(CLOSINGS));
static FOLDED_SUBJECTS: LazyLock<Vec<String>> = LazyLock::new(|| folded_pairs(SUBJECT_MARKERS));
static FOLDED_RECIPIENTS: LazyLock<Vec<String>> = LazyLock::new(|| folded_pairs(RECIPIENT_MARKERS));
static SUFFIX_KEYS: LazyLock<Vec<String>> = LazyLock::new(|| {
    COMPANY_SUFFIXES
        .iter()
        .map(|suffix| suffix.replace('.', ""))
        .collect()
});
/// The single-word degree keys and the degree phrases, folded.
static DEGREE_KEYS: LazyLock<(Vec<String>, Vec<String>)> = LazyLock::new(|| {
    let (phrases, words): (Vec<String>, Vec<String>) = DEGREE_WORDS
        .iter()
        .map(|(_, word)| token_key(word))
        .partition(|key| key.contains(' '));
    (words, phrases)
});
static FOLDED_DOCUMENT_TITLES: LazyLock<Vec<String>> =
    LazyLock::new(|| DOCUMENT_TITLES.iter().map(|title| fold(title)).collect());
static FOLDED_TITLES: LazyLock<Vec<(Language, String)>> = LazyLock::new(|| {
    TITLE_WORDS
        .iter()
        .map(|(language, word)| (*language, fold(word)))
        .collect()
});

/// The folded token with dots removed (`Sh.p.k.` is `shpk`).
fn token_key(token: &str) -> String {
    fold(token).replace('.', "")
}

/// The tokens of a line: split on whitespace and `,;:()/|`, with leading and trailing `.` trimmed.
fn tokens(line: &str) -> impl Iterator<Item = &str> {
    line.split(|c: char| c.is_whitespace() || ",;:()/|".contains(c))
        .map(|token| token.trim_matches('.'))
        .filter(|token| !token.is_empty())
}

/// The section kind a heading line names, if the whole line (without a trailing colon) is a
/// known heading in any language.
#[must_use]
pub fn heading_kind(line: &str) -> Option<SectionKind> {
    let folded = fold(line);
    let trimmed = folded.trim_matches(|c: char| c == ':' || c == '.' || c.is_whitespace());
    if trimmed.is_empty() {
        return None;
    }
    if FOLDED_DOCUMENT_TITLES.iter().any(|title| title == trimmed) {
        return None;
    }
    FOLDED_HEADINGS
        .iter()
        .find(|(_, phrase)| phrase == trimmed)
        .map(|(kind, _)| *kind)
}

/// Whether the line starts with a cover-letter salutation.
#[must_use]
pub fn is_salutation(line: &str) -> bool {
    let folded = fold(line);
    FOLDED_SALUTATIONS
        .iter()
        .any(|phrase| starts_with_phrase(&folded, phrase))
}

/// Whether the line starts with a cover-letter closing.
#[must_use]
pub fn is_closing(line: &str) -> bool {
    let folded = fold(line);
    FOLDED_CLOSINGS
        .iter()
        .any(|phrase| starts_with_phrase(&folded, phrase))
}

/// Whether the line starts with `marker` followed by a colon (a space may precede the colon).
fn starts_with_marker(line: &str, markers: &[String]) -> bool {
    let folded = fold(line);
    markers.iter().any(|marker| {
        folded
            .strip_prefix(marker.as_str())
            .is_some_and(|rest| rest.trim_start().starts_with(':'))
    })
}

/// Whether the line starts with a subject marker and a colon (`Subject:`, `Objet :`).
#[must_use]
pub fn is_subject(line: &str) -> bool {
    starts_with_marker(line, &FOLDED_SUBJECTS)
}

/// Whether the line starts with a recipient marker and a colon (`To:`, `À :`).
#[must_use]
pub fn is_recipient(line: &str) -> bool {
    starts_with_marker(line, &FOLDED_RECIPIENTS)
}

/// Whether a name segment of the line ends with a company legal-form suffix.
#[must_use]
pub fn has_company_suffix(line: &str) -> bool {
    line.split(|c: char| ",;|()".contains(c))
        .flat_map(|part| part.split(" - ").flat_map(|p| p.split(" \u{2013} ")))
        .any(|segment| {
            segment.split_whitespace().next_back().is_some_and(|last| {
                let key = last
                    .trim_matches(|c: char| !c.is_alphanumeric() && c != '.')
                    .replace('.', "");
                !key.is_empty() && SUFFIX_KEYS.contains(&key)
            })
        })
}

/// Whether the folded text holds `phrase` on word boundaries.
fn contains_phrase(folded: &str, phrase: &str) -> bool {
    folded.match_indices(phrase).any(|(start, found)| {
        let before = folded[..start].chars().next_back();
        let after = folded[start + found.len()..].chars().next();
        !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
    })
}

/// Whether the line holds a degree keyword (see [`DEGREE_WORDS`]). Plurals are explicit, not
/// prefixes: `Bachelors` matches, `Promotion`, `Mastered` and `Diplomat` do not.
#[must_use]
pub fn has_degree(line: &str) -> bool {
    let folded = fold(line);
    let (words, phrases) = &*DEGREE_KEYS;
    if phrases
        .iter()
        .any(|phrase| contains_phrase(&folded, phrase))
    {
        return true;
    }
    let raw: Vec<&str> = line
        .split(|c: char| c.is_whitespace() || ",;:()/|-".contains(c))
        .map(|token| token.trim_matches(['.', '\'', '"']))
        .filter(|token| !token.is_empty())
        .collect();
    raw.iter().enumerate().any(|(position, token)| {
        let key = token_key(token);
        let singular = key
            .strip_suffix("'s")
            .or_else(|| key.strip_suffix('s'))
            .unwrap_or(&key);
        let undotted = token.replace('.', "");
        let next = raw.get(position + 1).map(|t| fold(t));
        words.iter().any(|word| *word == key || word == singular)
            || DEGREE_CASED.contains(&undotted.as_str())
            || (DEGREE_AMBIGUOUS.contains(&undotted.as_str())
                && (token.contains('.')
                    || next.is_some_and(|n| matches!(n.as_str(), "in" | "of" | "en"))))
    })
}

/// Whether the line holds a job-title keyword in title position: a short line (up to six words), or
/// the first segment (before a comma, a bar, a dash or `at`, `chez`, `bei`, `në`, `for`) of a longer
/// one when that segment has at most five words; and the segment is written as a title, with at
/// least half of its words of three or more letters capitalized. `Lead a team of 5 engineers` and
/// `J'étais responsable de la migration` are sentences, not titles.
#[must_use]
pub fn has_title_keyword(line: &str) -> bool {
    let words = line.split_whitespace().count();
    let segment = if words <= 6 {
        line
    } else {
        let folded_cut = [
            " at ", " chez ", " bei ", " në ", " for ", " | ", " - ", ", ",
        ]
        .iter()
        .filter_map(|cut| line.find(cut))
        .min()
        .unwrap_or(line.len());
        &line[..folded_cut]
    };
    let segment_words: Vec<&str> = segment.split_whitespace().collect();
    if segment_words.len() > 6 || segment_words.is_empty() {
        return false;
    }
    let long: Vec<&&str> = segment_words
        .iter()
        .filter(|word| word.chars().filter(|c| c.is_alphabetic()).count() >= 3)
        .collect();
    let capitalized = long
        .iter()
        .filter(|word| word.chars().next().is_some_and(char::is_uppercase))
        .count();
    if long.is_empty() || capitalized * 2 < long.len() {
        return false;
    }
    tokens(segment).any(|token| {
        let key = fold(token);
        FOLDED_TITLES.iter().any(|(language, word)| {
            key == *word
                || (key.starts_with(word.as_str())
                    && key.chars().count() <= word.chars().count() + 1)
                || (*language == De
                    && key.chars().count() > word.chars().count()
                    && key.ends_with(word.as_str()))
        })
    })
}

/// Whether a token of the line is a street word.
#[must_use]
pub fn has_street_word(line: &str) -> bool {
    tokens(line).any(|token| {
        let key = fold(token);
        STREET_WORDS.iter().any(|(language, word)| {
            key == *word
                || (matches!(language, De | Sq)
                    && key.chars().count() > word.chars().count()
                    && key.ends_with(word))
        })
    })
}

/// The month number of a folded month name or abbreviation (without a trailing dot).
#[must_use]
pub fn month_number(folded: &str) -> Option<u8> {
    MONTHS
        .iter()
        .find(|(_, name, _)| *name == folded)
        .map(|(_, _, number)| *number)
}

/// The language-independent folded ongoing markers, longest first.
#[must_use]
pub fn ongoing_markers_longest_first() -> Vec<String> {
    let mut markers: Vec<String> = ONGOING_MARKERS.iter().map(|(_, m)| fold(m)).collect();
    markers.sort_by_key(|marker| std::cmp::Reverse(marker.chars().count()));
    markers
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_language_has_every_table_covered() {
        for language in Language::ALL {
            for kind in SectionKind::ALL {
                assert!(
                    HEADINGS
                        .iter()
                        .any(|(k, l, _)| *k == kind && *l == language),
                    "{language:?} {kind:?}"
                );
            }
            for table in [
                SALUTATIONS,
                CLOSINGS,
                SUBJECT_MARKERS,
                DEGREE_WORDS,
                TITLE_WORDS,
            ] {
                assert!(table.iter().any(|(l, _)| *l == language), "{language:?}");
            }
            assert!(MONTHS.iter().any(|(l, _, _)| *l == language));
            assert!(ONGOING_MARKERS.iter().any(|(l, _)| *l == language));
        }
    }

    #[test]
    fn folding_drops_accents_and_case() {
        assert_eq!(fold("  Përvoja   e Punës "), "pervoja e punes");
        assert_eq!(fold("Mit freundlichen Grüßen"), "mit freundlichen grussen");
        assert_eq!(fold("AUJOURD\u{2019}HUI"), "aujourd'hui");
    }
}
