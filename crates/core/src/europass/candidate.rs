//! Parser of the HR-XML based v4 `Candidate` CV (namespace `http://www.europass.eu/1.0`).
//!
//! Elements live in four namespaces: the default one (`EU`), `hr:` (`HR`), `oa:` (`OA`) and
//! `eures:` (`EURES`); the lookups below name the namespace of every element exactly as the editor
//! writes it. Unknown elements are ignored; non-empty section elements that are not mapped are
//! listed in `unsupported_sections`. Every list is capped in total, and every cap hit, malformed
//! value or dropped field is recorded under an indexed path (`employment[3].position`).

use super::model::*;
use super::xml::{Elem, Reader, at, child, children, has_content};

pub(super) const EU: &str = "http://www.europass.eu/1.0";
const HR: &str = "http://www.hr-xml.org/3";
const OA: &str = "http://www.openapplications.org/oagis/9";
const EURES: &str = "http://www.europass_eures.eu/1.0";

/// Profile children the parser maps (or that carry only identifiers).
const MAPPED_SECTIONS: [&str; 5] = [
    "ID",
    "ExecutiveSummary",
    "EmploymentHistory",
    "EducationHistory",
    "Skills",
];

pub(super) fn parse(root: Elem<'_, '_>) -> EuropassCv {
    let mut reader = Reader::default();

    let schema_version = reader.attr(
        child(root, HR, "DocumentID"),
        "schemeVersionID",
        "schemaVersion",
    );
    let person = match child(root, EU, "CandidatePerson") {
        Some(node) => person(&mut reader, node),
        None => Person::default(),
    };

    let mut cv = EuropassCv {
        schema_version,
        language_code: None,
        person,
        summary: None,
        employment: Vec::new(),
        education: Vec::new(),
        languages: Vec::new(),
        skills: Vec::new(),
        design: None,
        unsupported_sections: Vec::new(),
        truncated_fields: Vec::new(),
        invalid_fields: Vec::new(),
    };

    if let Some(profile) = child(root, EU, "CandidateProfile") {
        cv.language_code = reader.attr(Some(profile), "languageCode", "languageCode");
        cv.summary = reader.rich(child(profile, HR, "ExecutiveSummary"), "summary");
        if let Some(history) = child(profile, EU, "EmploymentHistory") {
            for employer in children(history, EU, "EmployerHistory") {
                employment(&mut reader, employer, &mut cv.employment);
            }
        }
        if let Some(history) = child(profile, EU, "EducationHistory") {
            for attendance in children(history, EU, "EducationOrganizationAttendance") {
                education(&mut reader, attendance, &mut cv.education);
            }
        }
        if let Some(qualifications) = child(profile, EU, "PersonQualifications") {
            for competency in children(qualifications, EU, "PersonCompetency") {
                if !reader.has_room(cv.languages.len(), "languages") {
                    break;
                }
                match language(&mut reader, competency, cv.languages.len()) {
                    Some(language) => cv.languages.push(language),
                    None => reader.note_unsupported("PersonQualifications"),
                }
            }
        }
        if let Some(skills) = child(profile, EU, "Skills") {
            for competency in children(skills, EU, "PersonCompetency") {
                if !reader.has_room(cv.skills.len(), "skills") {
                    break;
                }
                let path = at("skills", cv.skills.len(), "name");
                let name = reader
                    .short(child(competency, HR, "CompetencyName"), &path)
                    .or_else(|| reader.short(child(competency, EU, "CompetencyID"), &path));
                if let Some(name) = name {
                    let taxonomy = reader.short(
                        child(competency, HR, "TaxonomyID"),
                        &at("skills", cv.skills.len(), "taxonomy"),
                    );
                    cv.skills.push(Skill { taxonomy, name });
                }
            }
        }
        for section in profile.children().filter(Elem::is_element) {
            let name = section.tag_name().name();
            if !MAPPED_SECTIONS.contains(&name)
                && name != "PersonQualifications"
                && has_content(section)
            {
                reader.note_unsupported(name);
            }
        }
    }
    if let Some(rendering) = child(root, EU, "RenderingInformation")
        && let Some(design) = child(rendering, EU, "Design")
    {
        cv.design = Some(design_of(&mut reader, design));
    }

    cv.unsupported_sections = reader.unsupported;
    cv.truncated_fields = reader.truncated;
    cv.invalid_fields = reader.invalid;
    cv
}

fn person(reader: &mut Reader, node: Elem<'_, '_>) -> Person {
    let mut person = Person::default();
    if let Some(name) = child(node, EU, "PersonName") {
        person.given_name = reader.short(child(name, OA, "GivenName"), "person.givenName");
        person.family_name = reader.short(child(name, HR, "FamilyName"), "person.familyName");
    }
    for communication in children(node, EU, "Communication") {
        let channel = reader.short(
            child(communication, EU, "ChannelCode"),
            "person.communication.channel",
        );
        let use_code = reader.short(
            child(communication, EU, "UseCode"),
            "person.communication.useCode",
        );
        match channel.as_deref().map(str::to_ascii_lowercase).as_deref() {
            Some("email") => {
                let value = reader.short(child(communication, OA, "URI"), "person.emails");
                if let Some(value) = value {
                    reader.push(&mut person.emails, value, "person.emails");
                }
            }
            Some("web") => {
                let value = reader.short(child(communication, OA, "URI"), "person.websites");
                if let Some(value) = value {
                    reader.push(&mut person.websites, value, "person.websites");
                }
            }
            Some("telephone") => {
                let index = person.phones.len();
                let number = reader.short(
                    child(communication, OA, "DialNumber"),
                    &at("person.phones", index, "number"),
                );
                if let Some(number) = number {
                    let phone = Phone {
                        use_code,
                        country_dialing: reader.short(
                            child(communication, EU, "CountryDialing"),
                            &at("person.phones", index, "countryDialing"),
                        ),
                        number,
                        country_code: reader.short(
                            child(communication, EU, "CountryCode"),
                            &at("person.phones", index, "countryCode"),
                        ),
                    };
                    reader.push(&mut person.phones, phone, "person.phones");
                }
            }
            None => {
                if let Some(address) = child(communication, EU, "Address") {
                    let index = person.addresses.len();
                    let path = format!("person.addresses[{index}]");
                    let mut value = address_of(reader, address, &path);
                    value.kind = reader
                        .attr(Some(address), "type", &format!("{path}.kind"))
                        .or(use_code);
                    reader.push(&mut person.addresses, value, "person.addresses");
                }
            }
            Some(_) if has_content(communication) => reader.note_unsupported("Communication"),
            Some(_) => {}
        }
    }
    person.birth_date = reader.date(child(node, HR, "BirthDate"), "person.birthDate");
    person.primary_language = reader.short(
        child(node, EU, "PrimaryLanguageCode"),
        "person.primaryLanguage",
    );
    person
}

fn address_of(reader: &mut Reader, node: Elem<'_, '_>, path: &str) -> Address {
    let mut lines = Vec::new();
    for line in children(node, OA, "AddressLine") {
        if let Some(text) = reader.short(Some(line), &format!("{path}.lines")) {
            reader.push(&mut lines, text, &format!("{path}.lines"));
        }
    }
    Address {
        kind: None,
        lines,
        city: reader.short(child(node, OA, "CityName"), &format!("{path}.city")),
        postal_code: reader.short(child(node, OA, "PostalCode"), &format!("{path}.postalCode")),
        country_code: reader.short(
            child(node, EU, "CountryCode"),
            &format!("{path}.countryCode"),
        ),
        country_label: None,
    }
}

/// City and country code of an `OrganizationContact`.
fn organization_location(
    reader: &mut Reader,
    node: Elem<'_, '_>,
    path: &str,
) -> (Option<String>, Option<String>) {
    let address = child(node, EU, "OrganizationContact")
        .and_then(|contact| child(contact, EU, "Communication"))
        .and_then(|communication| child(communication, EU, "Address"));
    match address {
        Some(address) => (
            reader.short(child(address, OA, "CityName"), &format!("{path}.city")),
            reader.short(
                child(address, EU, "CountryCode"),
                &format!("{path}.countryCode"),
            ),
        ),
        None => (None, None),
    }
}

/// A period from a start and an end date (`FormattedDateTime` children in `ns`) and a flag.
fn period_of(
    reader: &mut Reader,
    node: Elem<'_, '_>,
    ns: &str,
    flag: Option<Elem<'_, '_>>,
    path: &str,
) -> Period {
    let formatted =
        |name: &str| child(node, ns, name).and_then(|d| child(d, HR, "FormattedDateTime"));
    Period {
        start: reader.date(formatted("StartDate"), &format!("{path}.period.start")),
        end: reader.date(formatted("EndDate"), &format!("{path}.period.end")),
        current: reader.boolean(flag, &format!("{path}.period.current")),
    }
}

fn employment(reader: &mut Reader, employer: Elem<'_, '_>, out: &mut Vec<Employment>) {
    let first = format!("employment[{}]", out.len());
    let name = reader.short(
        child(employer, HR, "OrganizationName"),
        &format!("{first}.employer"),
    );
    let (city, country) = organization_location(reader, employer, &format!("{first}.employer"));
    let mut seen = false;
    for position in children(employer, EU, "PositionHistory") {
        seen = true;
        if !reader.has_room(out.len(), "employment") {
            return;
        }
        let path = format!("employment[{}]", out.len());
        let period = match child(position, EURES, "EmploymentPeriod") {
            Some(period) => period_of(
                reader,
                period,
                EURES,
                child(period, HR, "CurrentIndicator"),
                &path,
            ),
            None => Period::default(),
        };
        let entry = Employment {
            employer: name.clone(),
            employer_city: city.clone(),
            employer_country_code: country.clone(),
            position: reader.short(
                child(position, EU, "PositionTitle"),
                &format!("{path}.position"),
            ),
            period,
            description: reader.rich(
                child(position, OA, "Description"),
                &format!("{path}.description"),
            ),
            city: reader.short(child(position, EU, "City"), &format!("{path}.city")),
            country_code: reader.short(
                child(position, EU, "Country"),
                &format!("{path}.countryCode"),
            ),
        };
        out.push(entry);
    }
    if !seen && name.is_some() && reader.has_room(out.len(), "employment") {
        out.push(Employment {
            employer: name,
            employer_city: city,
            employer_country_code: country,
            position: None,
            period: Period::default(),
            description: None,
            city: None,
            country_code: None,
        });
    }
}

fn education(reader: &mut Reader, node: Elem<'_, '_>, out: &mut Vec<Education>) {
    let first = format!("education[{}]", out.len());
    let organization = reader.short(
        child(node, HR, "OrganizationName"),
        &format!("{first}.organization"),
    );
    let (city, country_code) = organization_location(reader, node, &first);
    let period = match child(node, EU, "AttendancePeriod") {
        Some(period) => period_of(reader, period, EU, child(period, EU, "Ongoing"), &first),
        None => Period::default(),
    };
    let mut seen = false;
    for degree in children(node, EU, "EducationDegree") {
        seen = true;
        if !reader.has_room(out.len(), "education") {
            return;
        }
        let path = format!("education[{}]", out.len());
        let entry = Education {
            organization: organization.clone(),
            city: city.clone(),
            country_code: country_code.clone(),
            period: period.clone(),
            degree: reader.short(child(degree, HR, "DegreeName"), &format!("{path}.degree")),
            description: reader.rich(
                child(degree, EU, "OccupationalSkillsCovered"),
                &format!("{path}.description"),
            ),
        };
        out.push(entry);
    }
    if !seen
        && (organization.is_some() || period != Period::default())
        && reader.has_room(out.len(), "education")
    {
        out.push(Education {
            organization,
            city,
            country_code,
            period,
            degree: None,
            description: None,
        });
    }
}

/// A language competency; `None` when the competency is not a language one.
fn language(reader: &mut Reader, node: Elem<'_, '_>, index: usize) -> Option<LanguageSkill> {
    let taxonomy = reader.short(
        child(node, HR, "TaxonomyID"),
        &at("languages", index, "taxonomy"),
    )?;
    if !taxonomy.eq_ignore_ascii_case("language") {
        return None;
    }
    let id = child(node, EU, "CompetencyID");
    let name = reader.short(id, &at("languages", index, "name"))?;
    let mut skill = LanguageSkill {
        name,
        scheme: reader.attr(id, "schemeName", &at("languages", index, "scheme")),
        listening: None,
        reading: None,
        spoken_interaction: None,
        spoken_production: None,
        writing: None,
    };
    for dimension in children(node, EURES, "CompetencyDimension") {
        let code = reader.short(
            child(dimension, HR, "CompetencyDimensionTypeCode"),
            &at("languages", index, "dimension"),
        );
        let (field, slot) = match code.as_deref() {
            Some("CEF-Understanding-Listening") => ("listening", &mut skill.listening),
            Some("CEF-Understanding-Reading") => ("reading", &mut skill.reading),
            Some("CEF-Speaking-Interaction") => {
                ("spokenInteraction", &mut skill.spoken_interaction)
            }
            Some("CEF-Speaking-Production") => ("spokenProduction", &mut skill.spoken_production),
            Some("CEF-Writing-Production") => ("writing", &mut skill.writing),
            _ => continue,
        };
        let path = at("languages", index, field);
        let text = child(dimension, EURES, "Score")
            .and_then(|score| child(score, HR, "ScoreText"))
            .and_then(|text| reader.short(Some(text), &path));
        if let Some(text) = text {
            *slot = Cefr::parse(&text);
            if slot.is_none() {
                reader.note_invalid(&path);
            }
        }
    }
    Some(skill)
}

fn design_of(reader: &mut Reader, node: Elem<'_, '_>) -> Design {
    let mut design = Design {
        template: reader.short(child(node, EU, "Template"), "design.template"),
        color: reader.short(child(node, EU, "Color"), "design.color"),
        font_size: reader.short(child(node, EU, "FontSize"), "design.fontSize"),
        logo: reader.short(child(node, EU, "Logo"), "design.logo"),
        page_numbers: reader.boolean(child(node, EU, "PageNumbers"), "design.pageNumbers"),
        sections_order: Vec::new(),
    };
    if let Some(order) = child(node, EU, "SectionsOrder") {
        for section in children(order, EU, "Section") {
            let title = reader.short(child(section, EU, "Title"), "design.sectionsOrder");
            if let Some(title) = title {
                reader.push(&mut design.sections_order, title, "design.sectionsOrder");
            }
        }
    }
    design
}
