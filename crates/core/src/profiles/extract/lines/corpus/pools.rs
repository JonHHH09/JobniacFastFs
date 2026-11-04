//! Held-out vocabulary and extra content of the synthetic corpus.
//!
//! The headings, employers, titles, degrees, salutations, closings and subjects here are NOT in the
//! feature lexicons (`lexicon.rs`): a classifier that only learns the lexicon flags fails on them.
//! About a third of the generated lines of each of those kinds are drawn from these pools. A test
//! checks that none of them matches the lexicon predicates.

/// The held-out and extra pools of one language.
pub(super) struct Pools {
    /// Held-out headings in `SectionKind::ALL` order: summary, experience, education, skills,
    /// languages, projects, certifications, volunteering, publications, references, interests,
    /// contact.
    pub(super) held_headings: [&'static [&'static str]; 12],
    /// Lexicon headings of projects, certifications, volunteering, publications, references and
    /// contact.
    pub(super) std_extra: [&'static str; 6],
    pub(super) held_employers: &'static [&'static str],
    pub(super) held_titles: &'static [&'static str],
    pub(super) held_degrees: &'static [&'static str],
    /// Salutations with `{title}` and `{last}`.
    pub(super) held_salutations: &'static [&'static str],
    pub(super) held_closings: &'static [&'static str],
    /// Subject lines without a marker, with `{role}`.
    pub(super) held_subjects: &'static [&'static str],
    pub(super) skill_labels: [&'static str; 4],
    /// Words that may replace the dash of a date range.
    pub(super) range_words: &'static [&'static str],
    /// The word between a title and an employer (`at`).
    pub(super) at_word: &'static str,
    pub(super) doc_titles: &'static [&'static str],
    pub(super) project_names: &'static [&'static str],
    pub(super) cert_names: &'static [&'static str],
    pub(super) pub_titles: &'static [&'static str],
    pub(super) volunteer_orgs: &'static [&'static str],
    pub(super) volunteer_roles: &'static [&'static str],
    pub(super) extra_bullets: &'static [&'static str],
    /// `Page {p} of {n}` split as the word before and the word between.
    pub(super) page_words: [&'static str; 2],
}

/// Single stray words that scanners and templates leave on their own line.
pub(super) const STRAY_WORDS: &[&str] =
    &["Confidential", "Draft", "Photo", "Anhang", "Foto", "DRAFT"];

pub(super) const EN: Pools = Pools {
    held_headings: [
        &["Who I Am", "Snapshot", "Highlights"],
        &["Career History", "Where I've Worked", "Roles I've Held"],
        &["Academic Background", "Schooling", "Studies"],
        &["What I Bring", "Toolbox", "Core Strengths"],
        &["Spoken Languages", "Tongues"],
        &["Selected Work", "Side Projects"],
        &["Credentials", "Accreditations"],
        &["Giving Back", "Community Work"],
        &["Papers & Talks", "Writing"],
        &["Recommendations"],
        &["Outside Work", "Beyond the Desk"],
        &["Get in Touch", "Reach Me"],
    ],
    std_extra: [
        "Projects",
        "Certifications",
        "Volunteering",
        "Publications",
        "References",
        "Contact",
    ],
    held_employers: &[
        "Northbridge Studio",
        "Mockford & Sons",
        "The Sample Collective",
        "Demo Works",
        "Fixture Row",
        "Placeholder Partners",
    ],
    held_titles: &[
        "Platform Wizard",
        "Head of Platform",
        "Founding Hacker",
        "Staff Alchemist",
        "Pipeline Keeper",
        "Digital Craftsperson",
    ],
    held_degrees: &[
        "Computer Science (honours)",
        "Studies in Information Systems",
        "Software Engineering Program",
        "Applied Mathematics",
    ],
    held_salutations: &[
        "Hi {title} {last},",
        "Greetings {title} {last},",
        "Good morning {title} {last},",
    ],
    held_closings: &[
        "Thanks and talk soon,",
        "Warmly,",
        "With gratitude,",
        "All the best,",
        "Cheers,",
    ],
    held_subjects: &[
        "{role} - application",
        "My application, {role}",
        "Interested in the {role} opening",
    ],
    skill_labels: ["Programming", "Tools", "Data", "Methods"],
    range_words: &["to", "until"],
    at_word: "at",
    doc_titles: &["Resume", "Curriculum Vitae", "CV"],
    project_names: &[
        "Inventory Sync Service",
        "Open Transit Dashboard",
        "Recipe Index",
        "Log Viewer",
    ],
    cert_names: &[
        "Certified Systems Practitioner",
        "Cloud Fundamentals",
        "Secure Coding Practitioner",
    ],
    pub_titles: &[
        "Practical Notes on Batch Pipelines",
        "Testing Without Mocks",
        "Small Teams, Large Systems",
    ],
    volunteer_orgs: &[
        "Riverside Food Bank",
        "Neighborhood Code Club",
        "Community Garden Network",
    ],
    volunteer_roles: &["Volunteer Coordinator", "Mentor", "Treasurer"],
    extra_bullets: &[
        "Shipped weekly",
        "Ran the on-call rotation for a service with 99.9 percent availability and wrote the postmortems for every incident",
        "Cut cloud spend by a quarter",
        "Introduced property-based tests for the pricing rules, which found three long-standing rounding defects before release",
    ],
    page_words: ["Page", "of"],
};

pub(super) const FR: Pools = Pools {
    held_headings: [
        &["En bref", "Qui suis-je"],
        &["Parcours", "Où j'ai travaillé", "Postes occupés"],
        &["Cursus", "Parcours scolaire", "Diplômes obtenus"],
        &["Savoir-faire", "Atouts"],
        &["Langues parlées"],
        &["Réalisations"],
        &["Titres et accréditations"],
        &["Engagement social"],
        &["Articles et conférences"],
        &["Recommandations"],
        &["Hors travail", "Passions"],
        &["Pour me joindre"],
    ],
    std_extra: [
        "Projets",
        "Certifications",
        "Bénévolat",
        "Publications",
        "Références",
        "Coordonnées",
    ],
    held_employers: &[
        "Atelier Nordbrise",
        "Maison Échantillon",
        "Le Collectif Exemple",
        "Studio Maquette",
    ],
    held_titles: &[
        "Artisan du numérique",
        "Chef de plateforme",
        "Bricoleur de données",
        "Gardien des pipelines",
    ],
    held_degrees: &[
        "Informatique de gestion",
        "Génie logiciel (programme coopératif)",
        "Techniques de l'informatique",
    ],
    held_salutations: &[
        "Salut {title} {last},",
        "À l'attention de {title} {last},",
        "Équipe de recrutement,",
    ],
    held_closings: &[
        "Bien à vous,",
        "Amicalement,",
        "Au plaisir,",
        "Respectueusement,",
    ],
    held_subjects: &["{role} - candidature spontanée", "Ma candidature, {role}"],
    skill_labels: ["Programmation", "Outils", "Données", "Méthodes"],
    range_words: &["à", "au"],
    at_word: "chez",
    doc_titles: &["Curriculum vitæ", "CV", "Résumé"],
    project_names: &[
        "Service de synchronisation",
        "Tableau de bord du transport",
        "Index de recettes",
    ],
    cert_names: &[
        "Praticien certifié des systèmes",
        "Fondements du nuage",
        "Codage sécuritaire",
    ],
    pub_titles: &[
        "Notes pratiques sur les traitements par lots",
        "Tester sans simulacres",
    ],
    volunteer_orgs: &["Banque alimentaire du quartier", "Club de code jeunesse"],
    volunteer_roles: &["Coordonnateur bénévole", "Mentor", "Trésorier"],
    extra_bullets: &[
        "Livraisons hebdomadaires",
        "Assuré la garde d'un service disponible à 99,9 pour cent et rédigé les analyses de chaque incident",
        "Réduit les coûts infonuagiques d'un quart",
    ],
    page_words: ["Page", "sur"],
};

pub(super) const DE: Pools = Pools {
    held_headings: [
        &["Wer ich bin", "Auf einen Blick"],
        &[
            "Stationen",
            "Beruflicher Hintergrund",
            "Wo ich gearbeitet habe",
        ],
        &["Akademischer Hintergrund", "Bildungsweg"],
        &["Was ich mitbringe", "Stärken"],
        &["Gesprochene Sprachen"],
        &["Ausgewählte Arbeiten"],
        &["Nachweise", "Qualifikationen"],
        &["Soziales Engagement"],
        &["Aufsätze und Vorträge"],
        &["Empfehlungen"],
        &["Freizeit", "Außerhalb der Arbeit"],
        &["So erreichen Sie mich"],
    ],
    std_extra: [
        "Projekte",
        "Zertifikate",
        "Ehrenamt",
        "Veröffentlichungen",
        "Referenzen",
        "Kontakt",
    ],
    held_employers: &[
        "Nordbrise Werkstatt",
        "Beispiel & Söhne",
        "Das Muster-Kollektiv",
        "Probestein Atelier",
    ],
    held_titles: &[
        "Datenschmied",
        "Leitung Plattform",
        "Code-Handwerker",
        "Pipeline-Hüter",
    ],
    held_degrees: &[
        "Informatik (Staatsexamen)",
        "Wirtschaftsinformatik",
        "Softwaretechnik Studium",
    ],
    held_salutations: &[
        "Werte {title} {last},",
        "Moin {title} {last},",
        "Liebes Recruiting-Team,",
    ],
    held_closings: &["Beste Grüße,", "Herzlich,", "Bis bald,"],
    held_subjects: &["{role} - meine Bewerbung", "Interesse an der Stelle {role}"],
    skill_labels: ["Programmiersprachen", "Werkzeuge", "Daten", "Methoden"],
    range_words: &["bis"],
    at_word: "bei",
    doc_titles: &["Lebenslauf", "CV", "Curriculum Vitae"],
    project_names: &[
        "Bestandsabgleich-Dienst",
        "Offenes Verkehrs-Dashboard",
        "Rezeptverzeichnis",
    ],
    cert_names: &[
        "Zertifizierter Systempraktiker",
        "Cloud-Grundlagen",
        "Sichere Programmierung",
    ],
    pub_titles: &[
        "Praktische Notizen zu Batch-Pipelines",
        "Testen ohne Attrappen",
    ],
    volunteer_orgs: &["Tafel am Fluss", "Nachbarschafts-Programmierclub"],
    volunteer_roles: &["Koordinator", "Mentor", "Kassenwart"],
    extra_bullets: &[
        "Wöchentliche Auslieferung",
        "Bereitschaftsdienst für einen Dienst mit 99,9 Prozent Verfügbarkeit übernommen und zu jedem Vorfall eine Nachbetrachtung verfasst",
        "Cloud-Kosten um ein Viertel gesenkt",
    ],
    page_words: ["Seite", "von"],
};

pub(super) const SQ: Pools = Pools {
    held_headings: [
        &["Kush jam", "Në pak fjalë"],
        &["Ku kam punuar", "Rrugëtimi profesional", "Pozicionet"],
        &["Sfondi akademik", "Shkollimi", "Studimet"],
        &["Çfarë sjell", "Pikat e forta"],
        &["Gjuhët që flas"],
        &["Punë të zgjedhura"],
        &["Kredenciale"],
        &["Kontribut në komunitet"],
        &["Artikuj dhe fjalime"],
        &["Rekomandime"],
        &["Jashtë punës", "Pasionet"],
        &["Si të më gjeni"],
    ],
    std_extra: [
        "Projektet",
        "Certifikimet",
        "Vullnetarizmi",
        "Publikimet",
        "Referencat",
        "Kontakti",
    ],
    held_employers: &[
        "Atelieja Veriu",
        "Shembulli dhe Bijtë",
        "Kolektivi Provaj",
        "Studio Modeli",
    ],
    held_titles: &[
        "Mjeshtër i kodit",
        "Kryetar i platformës",
        "Ndërtues sistemesh",
        "Rojtar i rrjedhave",
    ],
    held_degrees: &[
        "Shkenca Kompjuterike",
        "Inxhinieri Softuerike (studime të plota)",
        "Informatikë Aplikative",
    ],
    held_salutations: &[
        "Tungjatjeta {title} {last},",
        "Mirëdita {title} {last},",
        "Ekipi i rekrutimit,",
    ],
    held_closings: &["Të fala,", "Me konsideratë,", "Gjithë të mirat,"],
    held_subjects: &["{role} - aplikimi im", "Interesim për vendin {role}"],
    skill_labels: ["Programimi", "Mjetet", "Të dhënat", "Metodat"],
    range_words: &["deri"],
    at_word: "në",
    doc_titles: &["CV", "Curriculum Vitae"],
    project_names: &[
        "Shërbimi i sinkronizimit",
        "Paneli i hapur i transportit",
        "Indeksi i recetave",
    ],
    cert_names: &[
        "Praktikant i certifikuar i sistemeve",
        "Bazat e resë kompjuterike",
        "Programim i sigurt",
    ],
    pub_titles: &[
        "Shënime praktike për rrjedhat me grumbuj",
        "Testim pa simulime",
    ],
    volunteer_orgs: &["Banka e ushqimit e lagjes", "Klubi i kodit për të rinj"],
    volunteer_roles: &["Koordinator vullnetar", "Mentor", "Arkëtar"],
    extra_bullets: &[
        "Dorëzime javore",
        "Mora përsipër rotacionin e gatishmërisë për një shërbim me disponueshmëri 99,9 për qind dhe shkrova analizën e çdo incidenti",
        "E uli koston e resë me një të katërtën",
    ],
    page_words: ["Faqja", "nga"],
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profiles::extract::lines::lexicon::{
        has_company_suffix, has_degree, has_title_keyword, heading_kind, is_closing, is_recipient,
        is_salutation, is_subject,
    };

    #[test]
    fn held_out_vocabulary_is_not_in_the_lexicons() {
        for pools in [&EN, &FR, &DE, &SQ] {
            for group in pools.held_headings {
                assert!(!group.is_empty());
                for heading in group {
                    assert!(heading_kind(heading).is_none(), "{heading}");
                }
            }
            for employer in pools.held_employers {
                assert!(!has_company_suffix(employer), "{employer}");
            }
            for title in pools.held_titles {
                assert!(!has_title_keyword(title), "{title}");
            }
            for degree in pools.held_degrees {
                assert!(!has_degree(degree), "{degree}");
            }
            for template in pools.held_salutations {
                let text = template
                    .replace("{title}", "Ms.")
                    .replace("{last}", "Testwell");
                assert!(!is_salutation(&text), "{text}");
            }
            for closing in pools.held_closings {
                assert!(!is_closing(closing), "{closing}");
            }
            for template in pools.held_subjects {
                let text = template.replace("{role}", "Engineer");
                assert!(!is_subject(&text) && !is_recipient(&text), "{text}");
            }
            // The standard extra headings are lexicon headings, and document titles are not.
            for heading in pools.std_extra {
                assert!(heading_kind(heading).is_some(), "{heading}");
            }
            for title in pools.doc_titles {
                assert!(heading_kind(title).is_none(), "{title}");
            }
        }
    }
}
