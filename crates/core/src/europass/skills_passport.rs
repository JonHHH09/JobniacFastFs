//! Parser of the legacy Europass v3.4 `SkillsPassport` cover letter (`DocumentType` `ECL`).
//!
//! Every element is in the single namespace `http://europass.cedefop.europa.eu/Europass`. The letter
//! date is stored as XSD `gDay`/`gMonth`/`gYear` attributes (`---03`, `--11`, `2026`). Caps,
//! malformed values and unmapped elements are recorded like in the v4 CV parser.

use super::model::*;
use super::xml::{Elem, Reader, child, children, has_content};

pub(super) const NS: &str = "http://europass.cedefop.europa.eu/Europass";

/// The `DocumentType` text of a v3 document, if present.
pub(super) fn document_type(root: Elem<'_, '_>) -> Option<String> {
    let info = child(root, NS, "DocumentInfo")?;
    let node = child(info, NS, "DocumentType")?;
    let text: String = node.children().filter_map(|c| c.text()).collect();
    Some(text.trim().to_owned())
}

/// Record the non-empty children of `node` whose name is not in `mapped`.
fn note_unmapped(reader: &mut Reader, node: Elem<'_, '_>, mapped: &[&str]) {
    for section in node.children().filter(Elem::is_element) {
        let name = section.tag_name().name();
        if !mapped.contains(&name) && has_content(section) {
            reader.note_unsupported(name);
        }
    }
}

pub(super) fn parse_cover_letter(root: Elem<'_, '_>) -> EuropassCoverLetter {
    let mut reader = Reader::default();

    let mut document_info = DocumentInfo::default();
    if let Some(info) = child(root, NS, "DocumentInfo") {
        document_info = DocumentInfo {
            creation_date: reader
                .short(child(info, NS, "CreationDate"), "documentInfo.creationDate"),
            last_update_date: reader.short(
                child(info, NS, "LastUpdateDate"),
                "documentInfo.lastUpdateDate",
            ),
            xsd_version: reader.short(child(info, NS, "XSDVersion"), "documentInfo.xsdVersion"),
            generator: reader.short(child(info, NS, "Generator"), "documentInfo.generator"),
        };
    }

    let mut sender = Person::default();
    if let Some(learner) = child(root, NS, "LearnerInfo") {
        if let Some(identification) = child(learner, NS, "Identification") {
            sender = identification_person(&mut reader, identification);
        }
        note_unmapped(&mut reader, learner, &["Identification"]);
    }

    let mut letter = EuropassCoverLetter {
        locale: reader.attr(Some(root), "locale", "locale"),
        document_info,
        sender,
        addressee: Addressee::default(),
        date: None,
        place: None,
        subject: None,
        opening: Salutation::default(),
        body: None,
        closing: None,
        design: None,
        unsupported_sections: Vec::new(),
        truncated_fields: Vec::new(),
        invalid_fields: Vec::new(),
    };

    if let Some(cover) = child(root, NS, "CoverLetter") {
        if let Some(addressee) = child(cover, NS, "Addressee") {
            letter.addressee = addressee_of(&mut reader, addressee);
        }
        if let Some(body) = child(cover, NS, "Letter") {
            letter_of(&mut reader, body, &mut letter);
        }
        if let Some(design) = child(cover, NS, "Design") {
            letter.design = Some(Design {
                template: reader.short(child(design, NS, "Template"), "design.template"),
                color: reader.short(child(design, NS, "Color"), "design.color"),
                font_size: reader.short(child(design, NS, "FontSize"), "design.fontSize"),
                logo: reader.short(child(design, NS, "Logo"), "design.logo"),
                page_numbers: reader
                    .boolean(child(design, NS, "PageNumbers"), "design.pageNumbers"),
                sections_order: Vec::new(),
            });
        }
        note_unmapped(&mut reader, cover, &["Addressee", "Letter", "Design"]);
    }

    letter.unsupported_sections = reader.unsupported;
    letter.truncated_fields = reader.truncated;
    letter.invalid_fields = reader.invalid;
    letter
}

fn letter_of(reader: &mut Reader, node: Elem<'_, '_>, letter: &mut EuropassCoverLetter) {
    if let Some(localisation) = child(node, NS, "Localisation") {
        if let Some(date) = child(localisation, NS, "Date") {
            let part = |name: &str| date.attribute(name);
            match PartialDate::from_g_parts(part("year"), part("month"), part("day")) {
                Ok(date) => letter.date = date,
                Err(InvalidDate) => reader.note_invalid("date"),
            }
        }
        letter.place = child(localisation, NS, "Place")
            .and_then(|place| reader.short(child(place, NS, "Municipality"), "place"));
    }
    letter.subject = reader.short(child(node, NS, "SubjectLine"), "subject");
    if let Some(opening) = child(node, NS, "OpeningSalutation") {
        letter.opening = Salutation {
            label: child(opening, NS, "Salutation")
                .and_then(|s| reader.short(child(s, NS, "Label"), "opening.label")),
            name: child(opening, NS, "PersonName").and_then(|name| {
                let surname = reader.short(child(name, NS, "Surname"), "opening.name");
                let first = reader.short(child(name, NS, "FirstName"), "opening.name");
                match (first, surname) {
                    (Some(first), Some(surname)) => Some(format!("{first} {surname}")),
                    (first, surname) => first.or(surname),
                }
            }),
        };
    }
    if let Some(body) = child(node, NS, "Body") {
        letter.body = reader.rich(child(body, NS, "MainBody"), "body");
        note_unmapped(reader, body, &["MainBody"]);
    }
    letter.closing = child(node, NS, "ClosingSalutation")
        .and_then(|closing| reader.short(child(closing, NS, "Label"), "closing"));
    note_unmapped(
        reader,
        node,
        &[
            "Localisation",
            "SubjectLine",
            "OpeningSalutation",
            "Body",
            "ClosingSalutation",
        ],
    );
}

/// `Contact` block of an address: `AddressLine`, `AddressLine2`, `PostalCode`, `Municipality`, `Country`.
fn address_of(reader: &mut Reader, address: Elem<'_, '_>, path: &str) -> Option<Address> {
    let contact = child(address, NS, "Contact")?;
    let lines: Vec<String> = ["AddressLine", "AddressLine2"]
        .iter()
        .filter_map(|name| reader.short(child(contact, NS, name), &format!("{path}.lines")))
        .collect();
    let country = child(contact, NS, "Country");
    let value = Address {
        kind: None,
        lines,
        city: reader.short(child(contact, NS, "Municipality"), &format!("{path}.city")),
        postal_code: reader.short(
            child(contact, NS, "PostalCode"),
            &format!("{path}.postalCode"),
        ),
        country_code: country
            .and_then(|c| reader.short(child(c, NS, "Code"), &format!("{path}.countryCode"))),
        country_label: country
            .and_then(|c| reader.short(child(c, NS, "Label"), &format!("{path}.countryLabel"))),
    };
    let empty = value.lines.is_empty()
        && value.city.is_none()
        && value.postal_code.is_none()
        && value.country_code.is_none()
        && value.country_label.is_none();
    (!empty).then_some(value)
}

fn names(reader: &mut Reader, node: Elem<'_, '_>, path: &str) -> (Option<String>, Option<String>) {
    match child(node, NS, "PersonName") {
        Some(name) => (
            reader.short(child(name, NS, "FirstName"), &format!("{path}.givenName")),
            reader.short(child(name, NS, "Surname"), &format!("{path}.familyName")),
        ),
        None => (None, None),
    }
}

fn identification_person(reader: &mut Reader, node: Elem<'_, '_>) -> Person {
    let (given_name, family_name) = names(reader, node, "sender");
    let mut person = Person {
        given_name,
        family_name,
        ..Person::default()
    };
    if let Some(info) = child(node, NS, "ContactInfo") {
        if let Some(address) =
            child(info, NS, "Address").and_then(|a| address_of(reader, a, "sender.addresses[0]"))
        {
            person.addresses.push(address);
        }
        let email = child(info, NS, "Email")
            .and_then(|e| reader.short(child(e, NS, "Contact"), "sender.emails"));
        if let Some(email) = email {
            person.emails.push(email);
        }
        let listed: Vec<Elem<'_, '_>> = child(info, NS, "WebsiteList")
            .map(|list| children(list, NS, "Website").collect())
            .unwrap_or_default();
        for site in listed.into_iter().chain(children(info, NS, "Website")) {
            if let Some(url) = reader.short(child(site, NS, "Contact"), "sender.websites") {
                reader.push(&mut person.websites, url, "sender.websites");
            }
        }
        if let Some(list) = child(info, NS, "TelephoneList") {
            for phone in children(list, NS, "Telephone") {
                let index = person.phones.len();
                let path = format!("sender.phones[{index}]");
                if let Some(number) =
                    reader.short(child(phone, NS, "Contact"), &format!("{path}.number"))
                {
                    let entry = Phone {
                        use_code: child(phone, NS, "Use").and_then(|u| {
                            reader.short(child(u, NS, "Code"), &format!("{path}.useCode"))
                        }),
                        country_dialing: None,
                        number,
                        country_code: None,
                    };
                    reader.push(&mut person.phones, entry, "sender.phones");
                }
            }
        }
    }
    person
}

fn addressee_of(reader: &mut Reader, node: Elem<'_, '_>) -> Addressee {
    let (given_name, family_name) = names(reader, node, "addressee");
    let organisation = child(node, NS, "Organisation");
    Addressee {
        given_name,
        family_name,
        organisation: organisation
            .and_then(|o| reader.short(child(o, NS, "Name"), "addressee.organisation")),
        address: organisation
            .and_then(|o| child(o, NS, "ContactInfo"))
            .and_then(|info| child(info, NS, "Address"))
            .and_then(|a| address_of(reader, a, "addressee.address")),
    }
}
