//! Fictional content pools of the synthetic corpus. Every name, company, street and sentence is
//! invented; emails and URLs are built from these on `example.com` and `example.org` only.

/// The content pools of one language.
pub(super) struct LangData {
    pub(super) first_names: &'static [&'static str],
    pub(super) last_names: &'static [&'static str],
    pub(super) titles: &'static [&'static str],
    pub(super) degrees: &'static [&'static str],
    pub(super) schools: &'static [&'static str],
    pub(super) bullets: &'static [&'static str],
    pub(super) summaries: &'static [&'static str],
    pub(super) skills: &'static [&'static str],
    /// Plain resume language lines (US, Canada, German Lebenslauf).
    pub(super) language_lines: &'static [&'static str],
    /// Europass language lines with a CEFR grid.
    pub(super) europass_language_lines: &'static [&'static str],
    pub(super) months_long: [&'static str; 12],
    pub(super) months_short: [&'static str; 12],
    /// Ongoing markers the generator may write.
    pub(super) ongoing: &'static [&'static str],
    pub(super) references_note: &'static str,
    pub(super) interests: &'static [&'static str],
    pub(super) company_suffixes: &'static [&'static str],
    // Letter pools.
    pub(super) letter_openings: &'static [&'static str],
    pub(super) letter_middles: &'static [&'static str],
    pub(super) letter_closings: &'static [&'static str],
    pub(super) letter_subject: &'static str,
    pub(super) letter_subject_marker: &'static str,
    pub(super) letter_recipient_marker: &'static str,
    pub(super) letter_enclosure: &'static str,
    pub(super) letter_titles: &'static [&'static str],
}

pub(super) const EN: LangData = LangData {
    first_names: &[
        "Alex", "Jordan", "Taylor", "Morgan", "Casey", "Riley", "Jamie", "Avery", "Quinn", "Robin",
    ],
    last_names: &[
        "Samplewood",
        "Testwell",
        "Exampleton",
        "Mockridge",
        "Demofield",
        "Placeholder",
        "Specimen",
        "Fixturewood",
    ],
    titles: &[
        "Software Engineer",
        "Senior Developer",
        "Data Analyst",
        "Project Manager",
        "Systems Administrator",
        "Technical Consultant",
        "QA Engineer",
        "Product Designer",
    ],
    degrees: &[
        "B.S. in Computer Science",
        "Bachelor of Science in Information Systems",
        "M.S. in Data Science",
        "Bachelor of Engineering in Software Engineering",
    ],
    schools: &[
        "Example University",
        "Sample State University",
        "Demo Institute of Technology",
        "Mockford College",
    ],
    bullets: &[
        "Built and maintained REST services used by 40 internal teams",
        "Reduced nightly batch time from six hours to ninety minutes",
        "Led a team of five engineers through two major releases",
        "Wrote automated tests that cut regression defects by a third",
        "Migrated legacy reports to a PostgreSQL data warehouse",
        "Mentored junior developers and ran weekly code reviews",
        "Documented APIs and onboarding guides for new hires",
        "Improved pipeline reliability with automated rollbacks",
    ],
    summaries: &[
        "Software engineer with experience in backend services and data pipelines.",
        "Focused on reliable systems, clear documentation and steady delivery.",
        "Comfortable working with product owners, designers and operations teams.",
        "Enjoys turning unclear requirements into simple, testable designs.",
    ],
    skills: &[
        "Rust",
        "Python",
        "TypeScript",
        "PostgreSQL",
        "Docker",
        "Kubernetes",
        "Git",
        "SQL",
        "React",
        "Linux",
        "Java",
        "Go",
    ],
    language_lines: &["English - Native", "French - Conversational"],
    europass_language_lines: &["English: Mother tongue", "French: B2 B2 B1 B1 B2"],
    months_long: [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ],
    months_short: [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ],
    ongoing: &["Present", "Current", "Now", "Ongoing"],
    references_note: "References available upon request.",
    interests: &["Hiking", "Chess", "Open-source software"],
    company_suffixes: &["Inc.", "LLC", "Corp.", "Ltd"],
    letter_openings: &[
        "I am writing to apply for the {role} position at {company}.",
        "I am excited to submit my application for the {role} role at {company}.",
    ],
    letter_middles: &[
        "Over the past {years} years I have designed, built and maintained software for teams of every size, and I enjoy turning unclear requirements into reliable systems.",
        "In my current work I lead small projects, review code and keep delivery predictable for the people who depend on it.",
        "I am comfortable working across the stack, from database design to the user interface, and I care about tests and documentation.",
    ],
    letter_closings: &[
        "I would welcome the chance to discuss how my experience fits your team. Thank you for your time and consideration.",
        "Thank you for considering my application. I look forward to speaking with you.",
    ],
    letter_subject: "Application for {role}",
    letter_subject_marker: "Subject:",
    letter_recipient_marker: "To:",
    letter_enclosure: "Enclosure: Resume",
    letter_titles: &["Ms.", "Mr.", "Dr."],
};

pub(super) const FR: LangData = LangData {
    first_names: &[
        "Camille",
        "Dominique",
        "Claude",
        "Sacha",
        "Morgan",
        "Alexis",
        "Charlie",
    ],
    last_names: &[
        "Exemplaire",
        "Echantillon",
        "Maquette",
        "Essaimont",
        "Modelet",
        "Fictiveau",
    ],
    titles: &[
        "Ingénieur logiciel",
        "Développeur principal",
        "Analyste de données",
        "Gestionnaire de projet",
        "Administrateur système",
        "Conseiller technique",
    ],
    degrees: &[
        "Baccalauréat en informatique",
        "Maîtrise en génie logiciel",
        "DEC en techniques de l'informatique",
        "Licence en systèmes d'information",
    ],
    schools: &[
        "Université Exemple",
        "Collège Modèle",
        "École Maquette de technologie",
    ],
    bullets: &[
        "Conçu et maintenu des services REST utilisés par 40 équipes",
        "Réduit le traitement de nuit de six heures à quatre-vingt-dix minutes",
        "Dirigé une équipe de cinq ingénieurs sur deux versions majeures",
        "Rédigé des tests automatisés qui réduisent les défauts d'un tiers",
        "Migré des rapports anciens vers un entrepôt PostgreSQL",
        "Encadré de jeunes développeurs et animé des revues de code",
        "Documenté les API et les guides d'accueil des nouveaux employés",
    ],
    summaries: &[
        "Ingénieur logiciel expérimenté en services et en flux de données.",
        "Attaché aux systèmes fiables, à la documentation claire et à la livraison régulière.",
        "À l'aise avec les propriétaires de produit, les concepteurs et l'exploitation.",
    ],
    skills: &[
        "Rust",
        "Python",
        "TypeScript",
        "PostgreSQL",
        "Docker",
        "Git",
        "SQL",
        "React",
        "Linux",
        "Java",
    ],
    language_lines: &["Français - langue maternelle", "Anglais - courant"],
    europass_language_lines: &["Langue maternelle : français", "Anglais : C1 C1 B2 B2 C1"],
    months_long: [
        "janvier",
        "février",
        "mars",
        "avril",
        "mai",
        "juin",
        "juillet",
        "août",
        "septembre",
        "octobre",
        "novembre",
        "décembre",
    ],
    months_short: [
        "janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.", "nov.",
        "déc.",
    ],
    ongoing: &[
        "présent",
        "aujourd'hui",
        "à ce jour",
        "en cours",
        "actuellement",
    ],
    references_note: "Références disponibles sur demande.",
    interests: &["Randonnée", "Échecs", "Logiciels libres"],
    company_suffixes: &["inc.", "Ltée", "SA", "SAS", "SARL"],
    letter_openings: &[
        "Je vous écris pour poser ma candidature au poste de {role} chez {company}.",
        "C'est avec enthousiasme que je soumets ma candidature au poste de {role} chez {company}.",
    ],
    letter_middles: &[
        "Depuis {years} ans, je conçois et maintiens des logiciels pour des équipes de toutes tailles, et j'aime transformer des besoins flous en systèmes fiables.",
        "Dans mon travail actuel, je dirige de petits projets, je révise le code et je rends les livraisons prévisibles.",
        "Je suis à l'aise sur toute la pile, de la base de données à l'interface, et je tiens aux tests et à la documentation.",
    ],
    letter_closings: &[
        "Je serais heureux de discuter de l'apport de mon expérience à votre équipe. Je vous remercie de votre attention.",
        "Je vous remercie d'avoir considéré ma candidature et espère vous rencontrer prochainement.",
    ],
    letter_subject: "Candidature au poste de {role}",
    letter_subject_marker: "Objet :",
    letter_recipient_marker: "À :",
    letter_enclosure: "Pièce jointe : curriculum vitæ",
    letter_titles: &["Madame", "Monsieur"],
};

pub(super) const DE: LangData = LangData {
    first_names: &["Alex", "Sascha", "Jona", "Robin", "Kim", "Luca", "Noa"],
    last_names: &[
        "Beispielmann",
        "Musterfeld",
        "Probestein",
        "Testhagen",
        "Demobach",
    ],
    titles: &[
        "Softwareentwickler",
        "Projektleiter",
        "Datenanalyst",
        "Systemadministrator",
        "Technischer Berater",
        "Werkstudent",
    ],
    degrees: &[
        "Bachelor of Science Informatik",
        "Master of Science Wirtschaftsinformatik",
        "Diplom-Informatiker",
        "Bachelor of Engineering Softwaretechnik",
    ],
    schools: &[
        "Technische Universität Beispielstadt",
        "Hochschule Musterfeld",
        "Universität Probestein",
    ],
    bullets: &[
        "Entwicklung und Betrieb von REST-Diensten für 40 interne Teams",
        "Laufzeit des nächtlichen Batchs von sechs Stunden auf 90 Minuten gesenkt",
        "Team aus fünf Entwicklern durch zwei große Releases geführt",
        "Automatisierte Tests geschrieben und Regressionsfehler deutlich reduziert",
        "Alte Berichte in ein PostgreSQL-Data-Warehouse überführt",
        "Nachwuchsentwickler betreut und wöchentliche Code-Reviews geleitet",
        "Schnittstellen und Einarbeitungsunterlagen dokumentiert",
    ],
    summaries: &[
        "Softwareentwickler mit Erfahrung in Backend-Diensten und Datenpipelines.",
        "Legt Wert auf zuverlässige Systeme, klare Dokumentation und verlässliche Lieferung.",
        "Arbeitet gern mit Produktverantwortlichen, Designern und dem Betrieb zusammen.",
    ],
    skills: &[
        "Rust",
        "Python",
        "TypeScript",
        "PostgreSQL",
        "Docker",
        "Git",
        "SQL",
        "React",
        "Linux",
        "Java",
    ],
    language_lines: &["Deutsch - Muttersprache", "Englisch - fließend"],
    europass_language_lines: &["Muttersprache: Deutsch", "Englisch: C1 C1 B2 B2 C1"],
    months_long: [
        "Januar",
        "Februar",
        "März",
        "April",
        "Mai",
        "Juni",
        "Juli",
        "August",
        "September",
        "Oktober",
        "November",
        "Dezember",
    ],
    months_short: [
        "Jan", "Feb", "Mär", "Apr", "Mai", "Jun", "Jul", "Aug", "Sep", "Okt", "Nov", "Dez",
    ],
    ongoing: &["heute", "derzeit", "aktuell", "jetzt"],
    references_note: "Referenzen auf Anfrage.",
    interests: &["Wandern", "Schach", "Freie Software"],
    company_suffixes: &["GmbH", "AG", "KG"],
    letter_openings: &[
        "hiermit bewerbe ich mich auf die Stelle als {role} bei der {company}.",
        "mit großem Interesse bewerbe ich mich als {role} bei der {company}.",
    ],
    letter_middles: &[
        "Seit {years} Jahren entwickle und betreue ich Software für Teams jeder Größe und verwandle gern unklare Anforderungen in zuverlässige Systeme.",
        "In meiner aktuellen Tätigkeit leite ich kleine Projekte, prüfe Code und sorge für planbare Lieferungen.",
        "Ich arbeite sicher vom Datenbankentwurf bis zur Benutzeroberfläche und lege Wert auf Tests und Dokumentation.",
    ],
    letter_closings: &[
        "Über ein persönliches Gespräch, in dem ich meine Erfahrung näher erläutern kann, freue ich mich sehr. Vielen Dank für Ihre Zeit.",
        "Vielen Dank für die Berücksichtigung meiner Bewerbung. Ich freue mich auf ein Gespräch.",
    ],
    letter_subject: "Bewerbung als {role}",
    letter_subject_marker: "Betreff:",
    letter_recipient_marker: "An:",
    letter_enclosure: "Anlage: Lebenslauf",
    letter_titles: &["Frau", "Herr"],
};

pub(super) const SQ: LangData = LangData {
    first_names: &[
        "Arben", "Drita", "Besa", "Dritan", "Era", "Flutura", "Gent", "Ilir",
    ],
    last_names: &["Shembulli", "Provaj", "Modeli", "Kampioni", "Testaj"],
    titles: &[
        "Inxhinier softueri",
        "Zhvillues kryesor",
        "Analist të dhënash",
        "Menaxher projekti",
        "Administrator sistemi",
        "Konsulent teknik",
    ],
    degrees: &[
        "Bachelor në Shkenca Kompjuterike",
        "Master në Inxhinieri Softuerike",
        "Diplomë në Informatikë",
        "Bachelor në Sisteme Informacioni",
    ],
    schools: &[
        "Universiteti i Shembullit",
        "Instituti Provaj i Teknologjisë",
        "Universiteti Modeli",
    ],
    bullets: &[
        "Ndërtova dhe mirëmbajta shërbime REST për 40 ekipe të brendshme",
        "E uli kohën e përpunimit natën nga gjashtë orë në nëntëdhjetë minuta",
        "Drejtova një ekip prej pesë inxhinierësh në dy versione të mëdha",
        "Shkrova teste automatike që ulën gabimet e regresionit",
        "Zhvendosa raportet e vjetra në një magazinë të dhënash PostgreSQL",
        "Mentorova zhvillues të rinj dhe zhvillova rishikime javore të kodit",
        "Dokumentova API-të dhe udhëzuesit për punonjësit e rinj",
    ],
    summaries: &[
        "Inxhinier softueri me përvojë në shërbime backend dhe rrjedha të dhënash.",
        "I përkushtuar ndaj sistemeve të qëndrueshme dhe dokumentimit të qartë.",
        "Punon lehtë me pronarët e produktit, projektuesit dhe ekipet e operimit.",
    ],
    skills: &[
        "Rust",
        "Python",
        "TypeScript",
        "PostgreSQL",
        "Docker",
        "Git",
        "SQL",
        "React",
        "Linux",
        "Java",
    ],
    language_lines: &["Shqip - gjuha amtare", "Anglisht - rrjedhshëm"],
    europass_language_lines: &["Gjuha amtare: shqip", "Anglisht: C1 C1 B2 B2 C1"],
    months_long: [
        "Janar", "Shkurt", "Mars", "Prill", "Maj", "Qershor", "Korrik", "Gusht", "Shtator",
        "Tetor", "Nëntor", "Dhjetor",
    ],
    months_short: [
        "Jan", "Shk", "Mar", "Pri", "Maj", "Qer", "Kor", "Gus", "Sht", "Tet", "Nën", "Dhj",
    ],
    ongoing: &["sot", "aktualisht", "në vazhdim", "deri më sot"],
    references_note: "Referencat jepen sipas kërkesës.",
    interests: &["Ecje", "Shah", "Programe me burim të hapur"],
    company_suffixes: &["Sh.p.k.", "Sh.a."],
    letter_openings: &[
        "Po ju shkruaj për t'u aplikuar për pozicionin {role} në {company}.",
        "Me shumë interes paraqes aplikimin tim për pozicionin {role} në {company}.",
    ],
    letter_middles: &[
        "Prej {years} vitesh ndërtoj dhe mirëmbaj softuer për ekipe të të gjitha përmasave dhe më pëlqen t'i kthej kërkesat e paqarta në sisteme të besueshme.",
        "Në punën time aktuale drejtoj projekte të vogla, rishikoj kodin dhe e mbaj të parashikueshme dorëzimin.",
        "Ndihem i sigurt nga projektimi i bazës së të dhënave deri te ndërfaqja, dhe i kushtoj rëndësi testeve dhe dokumentimit.",
    ],
    letter_closings: &[
        "Do të isha i lumtur të diskutoja se si përvoja ime i shërben ekipit tuaj. Ju faleminderit për kohën dhe vëmendjen.",
        "Ju faleminderit që e shqyrtuat aplikimin tim. Pres me interes një takim me ju.",
    ],
    letter_subject: "Aplikim për pozicionin {role}",
    letter_subject_marker: "Lënda:",
    letter_recipient_marker: "Për:",
    letter_enclosure: "Bashkëngjitur: CV",
    letter_titles: &["Znj.", "Z."],
};

/// Company name stems.
pub(super) const COMPANY_STEMS: &[&str] = &[
    "Example",
    "Sample",
    "Demo",
    "Mockworks",
    "Testbed",
    "Placeholder",
    "Fixture",
    "Specimen",
];
/// Company name nouns.
pub(super) const COMPANY_NOUNS: &[&str] = &[
    "Systems",
    "Labs",
    "Analytics",
    "Software",
    "Logistics",
    "Digital",
];

/// A US city: name, state code, ZIP.
pub(super) const US_CITIES: &[(&str, &str, &str)] = &[
    ("Springfield", "IL", "62701"),
    ("Portland", "OR", "97201"),
    ("Austin", "TX", "73301"),
    ("Columbus", "OH", "43004"),
    ("Madison", "WI", "53703"),
    ("Denver", "CO", "80202"),
];
/// A Canadian city: name, province code, postal code.
pub(super) const CA_CITIES: &[(&str, &str, &str)] = &[
    ("Toronto", "ON", "M5V 2T6"),
    ("Montréal", "QC", "H2X 1Y4"),
    ("Vancouver", "BC", "V6B 1A1"),
    ("Calgary", "AB", "T2P 1J9"),
    ("Québec", "QC", "G1R 4P5"),
];
/// A German city: name, postal code.
pub(super) const DE_CITIES: &[(&str, &str)] = &[
    ("Berlin", "10115"),
    ("Hamburg", "20095"),
    ("München", "80331"),
    ("Köln", "50667"),
    ("Leipzig", "04109"),
];
/// An Albanian city: name, postal code.
pub(super) const SQ_CITIES: &[(&str, &str)] = &[
    ("Tiranë", "1001"),
    ("Durrës", "2001"),
    ("Shkodër", "4001"),
    ("Vlorë", "9401"),
];
/// A French city: name, postal code.
pub(super) const FR_CITIES: &[(&str, &str)] =
    &[("Paris", "75001"), ("Lyon", "69001"), ("Lille", "59000")];

/// US and Canadian English street names.
pub(super) const EN_STREETS: &[&str] =
    &["Example Street", "Sample Avenue", "Demo Road", "Mock Lane"];
/// French street names.
pub(super) const FR_STREETS: &[&str] = &["rue Exemple", "avenue de l'Échantillon", "rue Maquette"];
/// German street names.
pub(super) const DE_STREETS: &[&str] = &["Beispielstraße", "Musterweg", "Probeallee"];
/// Albanian street names.
pub(super) const SQ_STREETS: &[&str] = &["Rruga Shembulli", "Bulevardi Provaj", "Rruga Modeli"];
