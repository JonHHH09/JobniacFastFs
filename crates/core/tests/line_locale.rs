//! Locale rules of the line reader: dates, ranges, ambiguity, phones, postal codes, regions,
//! locale hints and the lexicons, table-driven with exact expected outputs. All text is synthetic.

use jobniac_core::profiles::extract::lines::dates::{
    DateOutcome, RangeEnd, RangeOutcome, contains_date, contains_ongoing_marker, contains_year,
    find_date_range, parse_date_range, parse_date_text,
};
use jobniac_core::profiles::extract::lines::lexicon::{
    CLOSINGS, Language, MONTHS, SALUTATIONS, SectionKind, has_company_suffix, has_degree,
    has_title_keyword, heading_kind, is_closing, is_recipient, is_salutation, is_subject,
};
use jobniac_core::profiles::extract::lines::locale::{
    CA_PROVINCES, Country, LocaleHint, PhoneKind, PostalKind, US_STATES, detect_locale,
    find_phones, find_postal_codes, find_region,
};

use LocaleHint::{Ca, Eu, Unknown, Us};

fn date(text: &str, hint: LocaleHint) -> String {
    match parse_date_text(text, hint) {
        DateOutcome::Parsed(d) => format!("{:04}-{:?}-{:?}", d.year, d.month, d.day),
        DateOutcome::Ambiguous => "ambiguous".to_owned(),
        DateOutcome::NotADate => "none".to_owned(),
    }
}

fn range(text: &str, hint: LocaleHint) -> String {
    match parse_date_range(text, hint) {
        RangeOutcome::Range(r) => match r.end {
            RangeEnd::Ongoing => format!("{}..ongoing", r.start.iso()),
            RangeEnd::Date(end) => format!("{}..{}", r.start.iso(), end.iso()),
        },
        RangeOutcome::Until(end) => format!("until {}", end.iso()),
        RangeOutcome::Ambiguous => "ambiguous".to_owned(),
        RangeOutcome::NotARange => "none".to_owned(),
    }
}

#[test]
fn single_dates_in_every_form() {
    let table: &[(&str, LocaleHint, &str)] = &[
        ("2020", Unknown, "2020-None-None"),
        ("05/2020", Unknown, "2020-Some(5)-None"),
        ("5-2020", Unknown, "2020-Some(5)-None"),
        ("05.2020", Unknown, "2020-Some(5)-None"),
        ("2020-05", Unknown, "2020-Some(5)-None"),
        ("2020/05", Unknown, "2020-Some(5)-None"),
        ("2020.05", Unknown, "2020-Some(5)-None"),
        ("2020-05-17", Unknown, "2020-Some(5)-Some(17)"),
        ("17.05.2020", Unknown, "2020-Some(5)-Some(17)"),
        ("17/05/2020", Unknown, "2020-Some(5)-Some(17)"),
        ("05/17/2020", Unknown, "2020-Some(5)-Some(17)"),
        ("05/17/2020", Eu, "2020-Some(5)-Some(17)"),
        ("17/05/2020", Us, "2020-Some(5)-Some(17)"),
        ("Jan 2020", Unknown, "2020-Some(1)-None"),
        ("January 2020", Unknown, "2020-Some(1)-None"),
        ("Sept. 2020", Unknown, "2020-Some(9)-None"),
        ("Jan2020", Unknown, "2020-Some(1)-None"),
        ("15 January 2020", Unknown, "2020-Some(1)-Some(15)"),
        ("January 15, 2020", Unknown, "2020-Some(1)-Some(15)"),
        ("janvier 2020", Unknown, "2020-Some(1)-None"),
        ("févr. 2020", Unknown, "2020-Some(2)-None"),
        ("août 2020", Unknown, "2020-Some(8)-None"),
        ("1er janvier 2020", Unknown, "2020-Some(1)-Some(1)"),
        ("Januar 2020", Unknown, "2020-Some(1)-None"),
        ("März 2020", Unknown, "2020-Some(3)-None"),
        ("Mär 2020", Unknown, "2020-Some(3)-None"),
        ("Dezember 2020", Unknown, "2020-Some(12)-None"),
        ("15. Januar 2020", Unknown, "2020-Some(1)-Some(15)"),
        ("Janar 2020", Unknown, "2020-Some(1)-None"),
        ("Shk 2020", Unknown, "2020-Some(2)-None"),
        ("Nëntor 2020", Unknown, "2020-Some(11)-None"),
        ("Dhjetor 2020", Unknown, "2020-Some(12)-None"),
        ("Qershor 2020", Unknown, "2020-Some(6)-None"),
        ("Korrik 2020", Unknown, "2020-Some(7)-None"),
        ("Foo 2020", Unknown, "none"),
        ("1850", Unknown, "none"),
        ("2020-13", Unknown, "none"),
        ("31/02/2020", Eu, "none"),
        ("13/14/2020", Eu, "none"),
        ("", Unknown, "none"),
    ];
    for (text, hint, expected) in table {
        assert_eq!(date(text, *hint), *expected, "{text} / {hint:?}");
    }
}

#[test]
fn every_month_abbreviation_of_every_language_parses() {
    for (language, name, number) in MONTHS {
        for text in [
            format!("{name} 2020"),
            format!("{name}. 2020"),
            format!("{} 2020", name.to_uppercase()),
        ] {
            assert_eq!(
                date(&text, Unknown),
                format!("2020-Some({number})-None"),
                "{language:?} {text}"
            );
        }
    }
    // Written forms with their accents and capitals.
    for (text, month) in [
        ("Febr. 2020", 2),
        ("Gush 2020", 8),
        ("Korr 2020", 7),
        ("Fév. 2020", 2),
        ("Juill. 2020", 7),
        ("Mär 2020", 3),
        ("Mrz 2020", 3),
        ("Dez. 2020", 12),
        ("Nën 2020", 11),
        ("Sht 2020", 9),
    ] {
        assert_eq!(
            date(text, Unknown),
            format!("2020-Some({month})-None"),
            "{text}"
        );
    }
}

#[test]
fn ambiguous_numeric_dates_follow_the_locale_hint() {
    let table: &[(&str, LocaleHint, &str)] = &[
        ("03/04/2020", Eu, "2020-Some(4)-Some(3)"),
        ("03/04/2020", Us, "2020-Some(3)-Some(4)"),
        ("03/04/2020", Ca, "ambiguous"),
        ("03/04/2020", Unknown, "ambiguous"),
        ("03-04-2020", Unknown, "ambiguous"),
        ("03/03/2020", Ca, "2020-Some(3)-Some(3)"),
        ("03.04.2020", Unknown, "2020-Some(4)-Some(3)"),
        ("03.04.2020", Us, "2020-Some(4)-Some(3)"),
    ];
    for (text, hint, expected) in table {
        assert_eq!(date(text, *hint), *expected, "{text} / {hint:?}");
    }
    assert_eq!(
        range("03/04/2020 - 05/06/2020", Eu),
        "2020-04-03..2020-06-05"
    );
    assert_eq!(
        range("03/04/2020 - 05/06/2020", Us),
        "2020-03-04..2020-05-06"
    );
    assert_eq!(range("03/04/2020 - 05/06/2020", Ca), "ambiguous");
    assert_eq!(range("03/04/2020 - 05/06/2020", Unknown), "ambiguous");
}

#[test]
fn ranges_with_every_separator_and_language() {
    let table: &[(&str, &str)] = &[
        ("Jan 2020 - Mar 2021", "2020-01-01..2021-03-01"),
        ("Jan 2020 – Mar 2021", "2020-01-01..2021-03-01"),
        ("Jan 2020 — Mar 2021", "2020-01-01..2021-03-01"),
        ("Jan 2018 to Mar 2020", "2018-01-01..2020-03-01"),
        ("janvier 2018 à mars 2020", "2018-01-01..2020-03-01"),
        ("mars 2018 au juin 2020", "2018-03-01..2020-06-01"),
        ("Mär 2018 bis Dez 2020", "2018-03-01..2020-12-01"),
        ("Mars 2018 deri Dhjetor 2020", "2018-03-01..2020-12-01"),
        ("05/2018 – 06/2020", "2018-05-01..2020-06-01"),
        ("05.2018 – 06.2020", "2018-05-01..2020-06-01"),
        ("2018-05 – 2020-06", "2018-05-01..2020-06-01"),
        ("2018-05-2020-06", "2018-05-01..2020-06-01"),
        ("2018 – 2020", "2018-01-01..2020-01-01"),
        ("2018-2020", "2018-01-01..2020-01-01"),
        ("from Jan 2018 to Mar 2020", "2018-01-01..2020-03-01"),
        ("depuis janvier 2018 à mars 2020", "2018-01-01..2020-03-01"),
        ("von März 2018 bis Dezember 2020", "2018-03-01..2020-12-01"),
        ("nga Janar 2018 deri Dhjetor 2020", "2018-01-01..2020-12-01"),
        ("2021 – 2019", "none"),
        ("Jan 2020 \u{2010} Mar 2021", "2020-01-01..2021-03-01"),
        ("Jan 2020 \u{2011} Mar 2021", "2020-01-01..2021-03-01"),
        ("Jan 2020 \u{2012} Mar 2021", "2020-01-01..2021-03-01"),
        ("Jan 2020 \u{2212} Mar 2021", "2020-01-01..2021-03-01"),
        ("seit 2019", "2019-01-01..ongoing"),
        ("since 2019", "2019-01-01..ongoing"),
        ("depuis janvier 2019", "2019-01-01..ongoing"),
        ("von März 2019", "2019-03-01..ongoing"),
        ("jusqu'à 2020", "until 2020-01-01"),
        ("jusqu\u{2019}à mars 2020", "until 2020-03-01"),
        ("until Mar 2020", "until 2020-03-01"),
        ("bis Dezember 2020", "until 2020-12-01"),
        ("deri Dhjetor 2020", "until 2020-12-01"),
        ("hello world", "none"),
        ("2020", "none"),
    ];
    for (text, expected) in table {
        assert_eq!(range(text, Unknown), *expected, "{text}");
    }
}

#[test]
fn every_ongoing_marker_ends_a_range() {
    let table: &[(&str, &str)] = &[
        ("Jan 2020 - Present", "2020-01-01..ongoing"),
        ("Jan 2020 – Current", "2020-01-01..ongoing"),
        ("Jan 2020 — Now", "2020-01-01..ongoing"),
        ("Jan 2020 to present", "2020-01-01..ongoing"),
        ("janvier 2020 – présent", "2020-01-01..ongoing"),
        ("janvier 2020 – aujourd'hui", "2020-01-01..ongoing"),
        ("janvier 2020 – aujourd\u{2019}hui", "2020-01-01..ongoing"),
        ("janvier 2020 à ce jour", "2020-01-01..ongoing"),
        ("janvier 2020 au présent", "2020-01-01..ongoing"),
        ("Januar 2020 – heute", "2020-01-01..ongoing"),
        ("Januar 2020 bis heute", "2020-01-01..ongoing"),
        ("01.2020 – derzeit", "2020-01-01..ongoing"),
        ("01.2020 - aktuell", "2020-01-01..ongoing"),
        ("Janar 2020 – sot", "2020-01-01..ongoing"),
        ("Janar 2020 deri aktualisht", "2020-01-01..ongoing"),
        ("01/2020 – në vazhdim", "2020-01-01..ongoing"),
        ("2019 – PRESENT", "2019-01-01..ongoing"),
        ("nga 2019 deri më sot", "2019-01-01..ongoing"),
        ("2019 - deri më sot", "2019-01-01..ongoing"),
        ("Janar 2019 – tani", "2019-01-01..ongoing"),
        ("Januar 2019 – jetzt", "2019-01-01..ongoing"),
        ("Januar 2019 bis dato", "2019-01-01..ongoing"),
        ("janvier 2019 – actuellement", "2019-01-01..ongoing"),
        ("Jan 2019 to date", "2019-01-01..ongoing"),
        ("Jan 2019 - Ongoing", "2019-01-01..ongoing"),
    ];
    for (text, expected) in table {
        assert_eq!(range(text, Unknown), *expected, "{text}");
        assert!(contains_ongoing_marker(text), "{text}");
    }
    assert!(!contains_ongoing_marker("I will present my work now"));
    assert!(!contains_ongoing_marker(
        "Led the current migration in 2019"
    ));
    assert!(!contains_ongoing_marker(
        "The ongoing work of 2020 was hard"
    ));
    assert!(contains_ongoing_marker("Present"));
    assert!(contains_ongoing_marker("Software Engineer, since 2019"));
}

#[test]
fn ranges_are_found_inside_running_text() {
    let found = find_date_range("Software Engineer, Jan 2020 – Present", Unknown);
    assert_eq!(
        found.map(|outcome| match outcome {
            RangeOutcome::Range(r) => format!("{}..{:?}", r.start.iso(), r.end),
            other => format!("{other:?}"),
        }),
        Some("2020-01-01..Ongoing".to_owned())
    );
    assert!(find_date_range("Example GmbH, Berlin", Unknown).is_none());
    assert_eq!(
        find_date_range("Tel 03/04/2020 - 05/06/2020", Ca),
        Some(RangeOutcome::Ambiguous)
    );
    assert!(contains_date("since March 2019"));
    assert!(contains_date("17.05.2020"));
    assert!(!contains_date("Class of 2019"));
    assert!(contains_year("Class of 2019"));
    assert!(!contains_year("12345"));
}

fn phones(text: &str) -> Vec<(PhoneKind, String)> {
    find_phones(text)
        .into_iter()
        .map(|m| (m.kind, m.text))
        .collect()
}

#[test]
fn phone_forms() {
    use PhoneKind::{Europass, International, Nanp};
    let table: Vec<(&str, Vec<(PhoneKind, &str)>)> = vec![
        ("(202) 555-0147", vec![(Nanp, "(202) 555-0147")]),
        ("202-555-0147", vec![(Nanp, "202-555-0147")]),
        ("+1 202 555 0147", vec![(Nanp, "+1 202 555 0147")]),
        ("202.555.0147", vec![(Nanp, "202.555.0147")]),
        ("2025550147", vec![(Nanp, "2025550147")]),
        ("(+1) 2025550147", vec![(Europass, "(+1) 2025550147")]),
        ("(+49) 30 1234567", vec![(Europass, "(+49) 30 1234567")]),
        ("+49 30 1234567", vec![(International, "+49 30 1234567")]),
        (
            "+355 69 123 4567",
            vec![(International, "+355 69 123 4567")],
        ),
        (
            "Tel +49 30 1234567, mobile 202-555-0147",
            vec![(International, "+49 30 1234567"), (Nanp, "202-555-0147")],
        ),
        ("2019 - 2021", vec![]),
        ("12345", vec![]),
        ("123-555-0147", vec![]),
        ("(202) 055-0147", vec![]),
        ("202-155-0147", vec![]),
        (
            "+49 30 1234567 2020",
            vec![(International, "+49 30 1234567")],
        ),
        (
            "+355 69 000 0047 2019",
            vec![(International, "+355 69 000 0047")],
        ),
        ("+1 202 555 0147 2019", vec![(Nanp, "+1 202 555 0147")]),
        (
            "(+49) 30 1234567 2020",
            vec![(Europass, "(+49) 30 1234567")],
        ),
        (
            "+355 69 123 2020",
            vec![(International, "+355 69 123 2020")],
        ),
        ("x20255501479", vec![]),
    ];
    for (text, expected) in table {
        let expected: Vec<(PhoneKind, String)> = expected
            .into_iter()
            .map(|(kind, found)| (kind, found.to_owned()))
            .collect();
        assert_eq!(phones(text), expected, "{text}");
    }
}

fn postal(text: &str) -> Vec<(PostalKind, String)> {
    find_postal_codes(text)
        .into_iter()
        .map(|m| (m.kind, m.text))
        .collect()
}

#[test]
fn postal_codes() {
    use PostalKind::{CaPostal, Eu as EuCode, UsZip, UsZipPlus4};
    let table: Vec<(&str, Vec<(PostalKind, &str)>)> = vec![
        ("Springfield, IL 62701", vec![(UsZip, "62701")]),
        (
            "Springfield, IL 62701-1234",
            vec![(UsZipPlus4, "62701-1234")],
        ),
        ("Toronto, ON M5V 2T6", vec![(CaPostal, "M5V 2T6")]),
        ("Toronto ON m5v2t6", vec![(CaPostal, "m5v2t6")]),
        ("10115 Berlin", vec![(EuCode, "10115")]),
        ("Beispielstraße 12, 10115 Berlin", vec![(EuCode, "10115")]),
        ("Beispielstraße 12 10115 Berlin", vec![(EuCode, "10115")]),
        ("D-10115 Berlin", vec![(EuCode, "D-10115")]),
        ("CH-8001 Zürich", vec![(EuCode, "CH-8001")]),
        ("Rruga Shembulli 12, 1001 Tiranë", vec![(EuCode, "1001")]),
        ("2020 Software Engineer", vec![]),
        ("Jan 2020 Software Engineer", vec![]),
        ("62701", vec![]),
        ("ZZ 62701", vec![]),
        ("I2A 1B1", vec![]),
        ("D1A 1A1", vec![]),
        ("W1A 1A1", vec![]),
        ("M5D 2T6", vec![]),
        ("M5V 2Q6", vec![]),
        ("K1A 0B1", vec![(CaPostal, "K1A 0B1")]),
        ("1200 Market Street", vec![]),
        (
            "1234 Elm Street, Springfield, IL 62701",
            vec![(UsZip, "62701")],
        ),
        ("5000 Rue Sherbrooke", vec![]),
        ("1000 Hauptstraße", vec![]),
        ("4500 Main St", vec![]),
        ("Musterweg 29, 20095 Hamburg", vec![(EuCode, "20095")]),
        ("Rruga Shembulli 12, 2001 Durrës", vec![(EuCode, "2001")]),
        ("1000 Skopje", vec![(EuCode, "1000")]),
    ];
    for (text, expected) in table {
        let expected: Vec<(PostalKind, String)> = expected
            .into_iter()
            .map(|(kind, found)| (kind, found.to_owned()))
            .collect();
        assert_eq!(postal(text), expected, "{text}");
    }
}

#[test]
fn regions() {
    let table: &[(&str, Option<(Country, &str)>)] = &[
        ("Springfield, IL", Some((Country::Us, "IL"))),
        ("Springfield IL 62701", Some((Country::Us, "IL"))),
        ("Montréal (QC)", Some((Country::Ca, "QC"))),
        ("Calgary, AB T2P 1J9", Some((Country::Ca, "AB"))),
        ("Toronto, Ontario", Some((Country::Ca, "ON"))),
        ("Austin, Texas", Some((Country::Us, "TX"))),
        ("Skills, SQL", None),
        ("Example Ltd, SA", None),
        ("Berlin", None),
        ("Python, OR Java", None),
        ("Skills: Rust, OR", Some((Country::Us, "OR"))),
        ("Foo, Georgia Tech", None),
    ];
    for (text, expected) in table {
        let found = find_region(text).map(|r| (r.country, r.code));
        assert_eq!(found, *expected, "{text}");
    }
    assert_eq!(US_STATES.len(), 51);
    assert_eq!(CA_PROVINCES.len(), 13);
    for (code, _) in CA_PROVINCES {
        assert!(!US_STATES.iter().any(|(us, _)| us == code), "{code}");
    }
}

#[test]
fn locale_hints() {
    let table: &[(&str, LocaleHint)] = &[
        ("Alex Sample\nSpringfield, IL 62701\n(202) 555-0147", Us),
        ("Alex Sample\n(202) 555-0147", Us),
        ("Alex Sample\nToronto, ON M5V 2T6\n(416) 555-0123", Ca),
        ("Alex Sample\nMontréal (QC)\nCanada", Ca),
        ("Europass\nMother tongue(s): English", Eu),
        (
            "Alex Sample\nBeispielstraße 12, 10115 Berlin\n+49 30 23125 047",
            Eu,
        ),
        ("Language skills\nEnglish C1 C1 B2 B2 C1", Eu),
        ("Alex Sample\nJan 2020", Unknown),
        ("Improved usage and usability metrics", Unknown),
        ("Moved to the USA in 2019", Us),
        ("Prishtina\nKosovo", Eu),
        ("Skopje, North Macedonia", Eu),
        ("Podgorica\nMontenegro", Eu),
        ("", Unknown),
    ];
    for (text, expected) in table {
        assert_eq!(detect_locale(text), *expected, "{text}");
    }
}

#[test]
fn headings_in_four_languages() {
    use SectionKind::{
        Certifications, Contact, Education, Experience, Interests, Languages, Projects,
        Publications, References, Skills, Summary, Volunteering,
    };
    let table: &[(&str, Option<SectionKind>)] = &[
        ("Work Experience", Some(Experience)),
        ("WORK EXPERIENCE:", Some(Experience)),
        ("Expérience professionnelle", Some(Experience)),
        ("Experience professionnelle", Some(Experience)),
        ("Berufserfahrung", Some(Experience)),
        ("Përvoja e punës", Some(Experience)),
        ("PERVOJA E PUNES", Some(Experience)),
        ("ABOUT ME", Some(Summary)),
        ("Professional Summary", Some(Summary)),
        ("À propos de moi", Some(Summary)),
        ("Über mich", Some(Summary)),
        ("Rreth meje", Some(Summary)),
        ("EDUCATION & TRAINING", Some(Education)),
        ("Formation", Some(Education)),
        ("Ausbildung", Some(Education)),
        ("Arsimi dhe trajnimi", Some(Education)),
        ("DIGITAL SKILLS", Some(Skills)),
        ("Compétences", Some(Skills)),
        ("Kenntnisse", Some(Skills)),
        ("Aftësitë", Some(Skills)),
        ("LANGUAGE SKILLS", Some(Languages)),
        ("Mother tongue(s)", Some(Languages)),
        ("Langues", Some(Languages)),
        ("Sprachen", Some(Languages)),
        ("Gjuhët", Some(Languages)),
        ("Projects", Some(Projects)),
        ("Projekte", Some(Projects)),
        ("Certifications", Some(Certifications)),
        ("Zertifikate", Some(Certifications)),
        ("Volunteering", Some(Volunteering)),
        ("Bénévolat", Some(Volunteering)),
        ("Publications", Some(Publications)),
        ("Veröffentlichungen", Some(Publications)),
        ("References", Some(References)),
        ("Referencat", Some(References)),
        ("Interests", Some(Interests)),
        ("Hobbys", Some(Interests)),
        ("Contact", Some(Contact)),
        ("Kontaktdaten", Some(Contact)),
        ("Other language(s)", Some(Languages)),
        ("Computer skills", Some(Skills)),
        ("Driving licence", Some(Certifications)),
        ("Resume", None),
        ("CV", None),
        ("Curriculum Vitae", None),
        ("Lebenslauf", None),
        ("Career History", None),
        ("Academic Background", None),
        ("Experience with Rust", None),
        ("", None),
        (":", None),
    ];
    for (text, expected) in table {
        assert_eq!(heading_kind(text), *expected, "{text}");
    }
}

fn check(name: &str, predicate: fn(&str) -> bool, positive: &[&str], negative: &[&str]) {
    for text in positive {
        assert!(predicate(text), "{name} should match {text:?}");
    }
    for text in negative {
        assert!(!predicate(text), "{name} should not match {text:?}");
    }
}

#[test]
fn letter_markers() {
    check(
        "salutation",
        is_salutation,
        &[
            "Dear Ms. Example,",
            "To whom it may concern",
            "Madame, Monsieur,",
            "Bonjour Madame,",
            "Sehr geehrte Frau Beispiel,",
            "Guten Tag,",
            "I nderuar Z. Provaj,",
            "E nderuar Znj. Modeli,",
            "I/E nderuar Z. Provaj,",
        ],
        &["Dearly beloved", "Skills", "I am writing"],
    );
    check(
        "closing",
        is_closing,
        &[
            "Sincerely,",
            "Kind regards,",
            "Best regards,",
            "Yours faithfully,",
            "Cordialement,",
            "Veuillez agréer, Madame, Monsieur, mes salutations distinguées.",
            "Mit freundlichen Grüßen",
            "Mit freundlichen Grussen",
            "Me respekt,",
            "Përshëndetje,",
            "Je vous prie d'agréer, Madame, Monsieur",
            "Faleminderit,",
            "Mit besten Grüßen",
        ],
        &["Regarding my work", "Sincerity matters"],
    );
    check(
        "subject",
        is_subject,
        &[
            "Subject: Application",
            "Re: Application",
            "Objet : Candidature",
            "Betreff: Bewerbung",
            "Lënda: Aplikim",
        ],
        &["Reference letter", "Subjective"],
    );
    check(
        "recipient",
        is_recipient,
        &[
            "To: Ms. Example",
            "À : Madame Exemple",
            "An: Frau Beispiel",
            "Për: Znj. Modeli",
        ],
        &["Total: 5", "Tomorrow: later"],
    );
    for language in Language::ALL {
        assert!(SALUTATIONS.iter().any(|(l, _)| *l == language));
        assert!(CLOSINGS.iter().any(|(l, _)| *l == language));
    }
}

#[test]
fn company_degree_and_title_keywords() {
    check(
        "company suffix",
        has_company_suffix,
        &[
            "Example Inc.",
            "Example LLC",
            "Example Ltd",
            "Example Corp.",
            "Example GmbH",
            "Beispiel AG",
            "Exemple SA",
            "Exemple SAS",
            "Exemple SARL",
            "Shembulli Sh.p.k.",
            "Shembulli Sh.a.",
            "Exemple Ltée",
            "Example Systems, Inc.",
            "Example Ltd, Springfield",
        ],
        &[
            "Example Systems",
            "Sample Analytics",
            "Limited experience",
            "Example sa",
            "Worked with the AG team lead",
        ],
    );
    check(
        "degree",
        has_degree,
        &[
            "Bachelor of Science",
            "Master in Data Science",
            "B.Sc. Computer Science",
            "MSc Informatics",
            "Baccalauréat en informatique",
            "Maîtrise en génie logiciel",
            "DEC en techniques de l'informatique",
            "Diplom-Informatiker",
            "Bachelor Informatik",
            "Bachelor në Shkenca Kompjuterike",
            "Diplomë në Informatikë",
            "Associate degree in Nursing",
            "M.S. in Data Science",
            "MS in Data Science",
            "Bachelors degree",
            "Bachelor's thesis",
            "Licence en droit",
        ],
        &[
            "Dec 2020",
            "Software Engineer",
            "Promotion",
            "Mastered the stack",
            "Diplomat",
            "MS Office",
            "Boston, MA",
            "Maîtrise du temps",
            "Driving licence",
        ],
    );
    check(
        "title",
        has_title_keyword,
        &[
            "Software Engineer",
            "Senior Developer",
            "Ingénieur logiciel",
            "Softwareentwickler",
            "Projektleiter",
            "Inxhinier softueri",
            "Zhvillues kryesor",
            "Responsable de projet",
            "Software Engineer, Example Systems GmbH",
        ],
        &[
            "Example Systems",
            "Internal tools",
            "Lead a team of 5",
            "Lead a team of 5 engineers on the migration",
            "J'étais responsable de la migration des données",
            "responsable de la migration",
        ],
    );
}
