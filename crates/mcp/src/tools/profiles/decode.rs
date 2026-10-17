//! Argument decoding of the profile tools, in the shapes of the tool signatures.
//!
//! Every `TypedDict` argument is decoded key by key in declaration order, and each nested write
//! item is decoded with its own dictionary location (`request.contacts.0.id`), so that a rejected
//! call reports every issue with the location the SDK would give it. Nested values are checked for
//! shape only: the handlers pass the request object to the service, which canonicalizes it and
//! ignores keys it does not know.

use jobniac_core::ids::is_canonical_uuid_text;
use serde_json::Value;
use uuid::Uuid;

use crate::args::{
    ArgDecoder, ArgErrors, ArgIssue, OPTIONAL_NULLABLE, PageRequest, REQUIRED, REQUIRED_NULLABLE,
    list_of, page_request, strict_int, strict_string, typed_dict, uuid_arg,
};
use crate::registry::Arguments;

/// Decoded `list_profiles` arguments: the optional page limit and cursor.
pub type ListArguments = PageRequest;

/// Decode the optional `request` of `list_profiles`. An absent or null request is `{}`.
pub fn list_profiles(arguments: &Arguments) -> Result<ListArguments, ArgErrors> {
    page_request("list_profiles", arguments)
}

/// Decode the `profileId` of `get_profile`.
pub fn get_profile(arguments: &Arguments) -> Result<String, ArgErrors> {
    let mut decoder = ArgDecoder::new("get_profile");
    let profile_id = decoder.field(arguments, "", "profileId", REQUIRED, profile_id_arg);
    decoder.check()?;
    Ok(profile_id.into_value().unwrap_or_default())
}

/// Decode `profileId`, `expectedRevision` and `request` of `update_profile`.
pub fn update_profile(arguments: &Arguments) -> Result<(String, i64), ArgErrors> {
    let mut decoder = ArgDecoder::new("update_profile");
    let profile_id = decoder.field(arguments, "", "profileId", REQUIRED, profile_id_arg);
    let expected = decoder.field(arguments, "", "expectedRevision", REQUIRED, strict_int);
    decoder.field(arguments, "", "request", REQUIRED, write_request);
    decoder.check()?;
    Ok((
        profile_id.into_value().unwrap_or_default(),
        expected.into_value().unwrap_or_default(),
    ))
}

/// Decode the `request` of `create_profile`.
pub fn create_profile(arguments: &Arguments) -> Result<(), ArgErrors> {
    let mut decoder = ArgDecoder::new("create_profile");
    decoder.field(arguments, "", "request", REQUIRED, write_request);
    decoder.check()
}

/// Decode the `request` of `update_profile_project` (`ProjectUpdateRequest`).
pub fn update_profile_project(arguments: &Arguments) -> Result<(), ArgErrors> {
    let mut decoder = ArgDecoder::new("update_profile_project");
    decoder.field(
        arguments,
        "",
        "request",
        REQUIRED,
        |location, value, issues| {
            typed_dict(location, value, issues, |decoder, object, parent| {
                decoder.field(object, parent, "profileId", REQUIRED, strict_string);
                decoder.field(object, parent, "projectId", REQUIRED, strict_string);
                decoder.field(object, parent, "expectedRevision", REQUIRED, strict_int);
                decoder.field(object, parent, "name", OPTIONAL_NULLABLE, strict_string);
                decoder.field(object, parent, "url", OPTIONAL_NULLABLE, strict_string);
                decoder.field(
                    object,
                    parent,
                    "description",
                    OPTIONAL_NULLABLE,
                    strict_string,
                );
                decoder.field(
                    object,
                    parent,
                    "displayOrder",
                    OPTIONAL_NULLABLE,
                    strict_int,
                );
                decoder.field(
                    object,
                    parent,
                    "technologies",
                    OPTIONAL_NULLABLE,
                    |location, value, issues| list_of(location, value, issues, technology_item),
                );
            })
        },
    );
    decoder.check()
}

/// Decode the `request` of `search_profiles` (`ProfileSearchRequest`, both keys required).
pub fn search_profiles(arguments: &Arguments) -> Result<(), ArgErrors> {
    let mut decoder = ArgDecoder::new("search_profiles");
    decoder.field(
        arguments,
        "",
        "request",
        REQUIRED,
        |location, value, issues| {
            typed_dict(location, value, issues, |decoder, object, parent| {
                decoder.field(object, parent, "query", REQUIRED_NULLABLE, strict_string);
                decoder.field(object, parent, "limit", REQUIRED_NULLABLE, strict_int);
            })
        },
    );
    decoder.check()
}

/// A profile identifier (`UUID` in the signature of `get_profile` and `update_profile`).
///
/// A string that the `UUID` type accepts is passed on in the normalized form when it has
/// the canonical shape, and as written otherwise. The service then admits it as a UUID, so a
/// braced, URN or compact spelling is refused there with `validation_error`. A string the
/// type rejects, and any non-string, is an SDK argument error.
fn profile_id_arg(location: &str, value: &Value, issues: &mut Vec<ArgIssue>) -> Option<String> {
    match value.as_str() {
        Some(text) if Uuid::parse_str(text).is_ok() => Some(if is_canonical_uuid_text(text) {
            text.to_ascii_lowercase()
        } else {
            text.to_owned()
        }),
        _ => {
            uuid_arg(location, value, issues);
            None
        }
    }
}

/// `ProfileWriteRequest`: ten keys, each required and nullable, with the write items of the lists.
fn write_request(location: &str, value: &Value, issues: &mut Vec<ArgIssue>) -> Option<()> {
    typed_dict(location, value, issues, |decoder, object, parent| {
        decoder.field(object, parent, "fullName", REQUIRED_NULLABLE, strict_string);
        decoder.field(object, parent, "email", REQUIRED_NULLABLE, strict_string);
        decoder.field(object, parent, "summary", REQUIRED_NULLABLE, strict_string);
        for (key, item) in WRITE_LISTS {
            decoder.field(
                object,
                parent,
                key,
                REQUIRED_NULLABLE,
                |location, value, issues| list_of(location, value, issues, item),
            );
        }
    })
}

/// The list keys of `ProfileWriteRequest`, in declaration order, with their item decoder.
const WRITE_LISTS: [(&str, Item); 7] = [
    ("contacts", contact_item),
    ("links", link_item),
    ("skills", skill_item),
    ("languages", language_item),
    ("education", education_item),
    ("experiences", experience_item),
    ("projects", project_item),
];

/// A decoder of one list item: the item location, its value and the issue sink.
type Item = fn(&str, &Value, &mut Vec<ArgIssue>) -> Option<()>;

/// `ContactWrite`: `id`, `contactType`, `contactValue` and `label`, each required and nullable.
fn contact_item(location: &str, value: &Value, issues: &mut Vec<ArgIssue>) -> Option<()> {
    typed_dict(location, value, issues, |decoder, object, parent| {
        for key in ["id", "contactType", "contactValue", "label"] {
            decoder.field(object, parent, key, REQUIRED_NULLABLE, strict_string);
        }
    })
}

/// `LinkWrite`: `id`, `linkType`, `url` and `label`, each required and nullable.
fn link_item(location: &str, value: &Value, issues: &mut Vec<ArgIssue>) -> Option<()> {
    typed_dict(location, value, issues, |decoder, object, parent| {
        for key in ["id", "linkType", "url", "label"] {
            decoder.field(object, parent, key, REQUIRED_NULLABLE, strict_string);
        }
    })
}

/// `SkillWrite`: the text keys are nullable, `displayOrder` is a required integer.
fn skill_item(location: &str, value: &Value, issues: &mut Vec<ArgIssue>) -> Option<()> {
    typed_dict(location, value, issues, |decoder, object, parent| {
        for key in ["id", "skill", "normalizedSkill", "category"] {
            decoder.field(object, parent, key, REQUIRED_NULLABLE, strict_string);
        }
        decoder.field(object, parent, "displayOrder", REQUIRED, strict_int);
    })
}

/// `LanguageWrite`: the text keys are nullable, `displayOrder` is a required integer.
fn language_item(location: &str, value: &Value, issues: &mut Vec<ArgIssue>) -> Option<()> {
    typed_dict(location, value, issues, |decoder, object, parent| {
        for key in ["id", "language", "normalizedLanguage", "proficiency"] {
            decoder.field(object, parent, key, REQUIRED_NULLABLE, strict_string);
        }
        decoder.field(object, parent, "displayOrder", REQUIRED, strict_int);
    })
}

/// `EducationWrite`: every key is a required nullable string.
fn education_item(location: &str, value: &Value, issues: &mut Vec<ArgIssue>) -> Option<()> {
    typed_dict(location, value, issues, |decoder, object, parent| {
        for key in [
            "id",
            "institution",
            "degree",
            "field",
            "location",
            "startDate",
            "endDate",
            "relevantFocus",
        ] {
            decoder.field(object, parent, key, REQUIRED_NULLABLE, strict_string);
        }
    })
}

/// `ExperienceWrite`: the text keys are nullable, `displayOrder` is a required integer.
fn experience_item(location: &str, value: &Value, issues: &mut Vec<ArgIssue>) -> Option<()> {
    typed_dict(location, value, issues, |decoder, object, parent| {
        for key in [
            "id",
            "company",
            "title",
            "location",
            "startDate",
            "endDate",
            "description",
        ] {
            decoder.field(object, parent, key, REQUIRED_NULLABLE, strict_string);
        }
        decoder.field(object, parent, "displayOrder", REQUIRED, strict_int);
    })
}

/// `ProjectWrite`: the text keys are nullable, `displayOrder` is required, `technologies` is a
/// nullable list.
fn project_item(location: &str, value: &Value, issues: &mut Vec<ArgIssue>) -> Option<()> {
    typed_dict(location, value, issues, |decoder, object, parent| {
        for key in ["id", "name", "url", "description"] {
            decoder.field(object, parent, key, REQUIRED_NULLABLE, strict_string);
        }
        decoder.field(object, parent, "displayOrder", REQUIRED, strict_int);
        decoder.field(
            object,
            parent,
            "technologies",
            REQUIRED_NULLABLE,
            |location, value, issues| list_of(location, value, issues, technology_item),
        );
    })
}

/// `TechnologyWrite`: `id` and `normalizedTechnology` and `technology` are nullable strings,
/// `displayOrder` is a required integer.
fn technology_item(location: &str, value: &Value, issues: &mut Vec<ArgIssue>) -> Option<()> {
    typed_dict(location, value, issues, |decoder, object, parent| {
        for key in ["id", "technology", "normalizedTechnology"] {
            decoder.field(object, parent, key, REQUIRED_NULLABLE, strict_string);
        }
        decoder.field(object, parent, "displayOrder", REQUIRED, strict_int);
    })
}

#[cfg(test)]
mod tests {
    use serde_json::{Map, json};

    use super::*;
    use crate::args::IssueKind;

    fn arguments(value: Value) -> Arguments {
        match value {
            Value::Object(map) => map,
            _ => Map::new(),
        }
    }

    fn kinds(errors: &ArgErrors) -> Vec<(String, IssueKind)> {
        errors
            .issues()
            .iter()
            .map(|issue| (issue.location().to_owned(), issue.kind()))
            .collect()
    }

    #[test]
    fn list_arguments_treat_an_absent_or_null_request_as_empty() {
        assert_eq!(
            list_profiles(&arguments(json!({}))).ok(),
            Some(ListArguments {
                limit: None,
                cursor: None
            })
        );
        assert!(list_profiles(&arguments(json!({"request": null}))).is_ok());
    }

    #[test]
    fn profile_ids_admit_canonical_text_and_defer_other_spellings() {
        let upper = arguments(json!({"profileId": "00000000-0000-4000-8000-0000000000AA"}));
        assert_eq!(
            get_profile(&upper).ok().as_deref(),
            Some("00000000-0000-4000-8000-0000000000aa")
        );
        let braced = arguments(json!({"profileId": "{00000000-0000-4000-8000-0000000000aa}"}));
        assert_eq!(
            get_profile(&braced).ok().as_deref(),
            Some("{00000000-0000-4000-8000-0000000000aa}")
        );
        let garbage = get_profile(&arguments(json!({"profileId": "not-a-uuid"})));
        assert_eq!(
            garbage.err().map(|errors| kinds(&errors)),
            Some(vec![("profileId".to_owned(), IssueKind::UuidParsing)])
        );
    }

    #[test]
    fn nested_write_issues_carry_their_list_index() {
        let request = json!({
            "fullName": "Example", "email": "x@example.test", "summary": null,
            "contacts": [{"id": null, "contactType": null, "contactValue": null}],
            "links": [5], "skills": null, "languages": null, "education": null,
            "experiences": null,
            "projects": [{"id": null, "name": null, "url": null, "description": null,
                          "displayOrder": 0, "technologies": ["Rust"]}],
        });
        let errors = create_profile(&arguments(json!({ "request": request })));
        let Err(errors) = errors else {
            panic!("the nested issues must reject the call");
        };
        assert_eq!(
            kinds(&errors),
            vec![
                ("request.contacts.0.label".to_owned(), IssueKind::Missing),
                ("request.links.0".to_owned(), IssueKind::DictType),
                (
                    "request.projects.0.technologies.0".to_owned(),
                    IssueKind::DictType
                ),
            ]
        );
    }

    #[test]
    fn a_write_request_with_every_key_is_accepted() {
        let nulls = json!({
            "fullName": null, "email": null, "summary": null, "contacts": null,
            "links": null, "skills": null, "languages": null, "education": null,
            "experiences": null, "projects": null,
        });
        assert!(create_profile(&arguments(json!({ "request": nulls }))).is_ok());
    }
}
