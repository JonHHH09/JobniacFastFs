//! Identifier admission and deterministic UUIDv3 identities.
//!
//! Admission: an identifier is accepted only in the 36-character hyphenated form, with hexadecimal
//! digits in either case, and it is normalized to lowercase. Braced, URN, simple (32 digit), a
//! non-ASCII digit and any other spelling is refused.
//!
//! The refusal is typed ([`UuidRefusal`]) because the boundaries do not share one envelope.
//! Each boundary maps the refusal to its own error:
//!
//! | Boundary                                          | absent / JSON null                 | malformed                          |
//! |---------------------------------------------------|------------------------------------|------------------------------------|
//! | entity deletion, profile details                 | `validation_error(m, f, "must be a valid UUID")` | same |
//! | generation (generic and German generators)      | `validation_error(m, f, "must not be null")` | `validation_error(m, f, "must be a valid UUID")` |
//! | matching                                          | `DomainValue("<f> must not be null")` | `DomainValue("<f> must be a valid UUID")` |
//!
//! Cursor payload identifiers are refused as `invalid_cursor` (see `pagination`).
//!
//! Name-based identities: the match report, review and disagreement identities are
//! `UUID(bytes=md5(identity).digest(), version=3)`. That is MD5 with the version and RFC
//! variant bits set, with no namespace, so it is built with `Builder::from_md5_bytes` and never with
//! `Uuid::new_v3`, which hashes a namespace and a name.

use md5::{Digest, Md5};
use uuid::{Builder, Uuid};

use crate::errors::{AppError, validation_error};

/// The reason text for a malformed identifier (`details.reason`).
pub const INVALID_UUID_REASON: &str = "must be a valid UUID";
/// The reason text for an absent identifier at the boundaries that distinguish it.
pub const NULL_UUID_REASON: &str = "must not be null";

/// Why an identifier was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UuidRefusal {
    /// The value was absent or JSON null.
    Null,
    /// The value is present but is not a canonical UUID (or is not a string).
    Malformed,
}

impl UuidRefusal {
    /// The `reason` detail text of this refusal.
    pub const fn reason(self) -> &'static str {
        match self {
            Self::Null => NULL_UUID_REASON,
            Self::Malformed => INVALID_UUID_REASON,
        }
    }

    /// The `validation_error` envelope of boundaries that use `field`/`reason` details.
    pub fn validation(self, message: &str, field: &str) -> AppError {
        AppError::from(validation_error(message, Some(field), Some(self.reason())))
    }

    /// The `DomainValue` error of the matching boundary (`<field> <reason>`, no details).
    pub fn domain_value(self, field: &str) -> AppError {
        AppError::DomainValue(format!("{field} {}", self.reason()))
    }
}

/// Whether `text` has the canonical shape: 36 bytes, hyphens at positions 8, 13, 18 and 23, and
/// ASCII hexadecimal digits (either case) everywhere else.
pub fn is_canonical_uuid_text(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => *byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        })
}

/// Admit a present identifier. The result is normalized to lowercase by
/// [`Uuid`]'s display form.
pub fn admit_uuid(text: &str) -> Result<Uuid, UuidRefusal> {
    if !is_canonical_uuid_text(text) {
        return Err(UuidRefusal::Malformed);
    }
    Uuid::parse_str(text).map_err(|_| UuidRefusal::Malformed)
}

/// Admit an optional identifier: `None` (absent or JSON null) is [`UuidRefusal::Null`].
pub fn admit_optional_uuid(value: Option<&str>) -> Result<Uuid, UuidRefusal> {
    match value {
        None => Err(UuidRefusal::Null),
        Some(text) => admit_uuid(text),
    }
}

/// Deterministic UUIDv3 identity of `identity` (its UTF-8 bytes):
/// `UUID(bytes=md5(identity).digest(), version=3)`.
pub fn name_uuid(identity: &str) -> Uuid {
    let digest = Md5::digest(identity.as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest);
    Builder::from_md5_bytes(bytes).into_uuid()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admits_only_the_hyphenated_form_in_either_case() {
        for text in [
            "0f0e0d0c-0b0a-4908-8706-050403020100",
            "0F0E0D0C-0B0A-4908-8706-050403020100",
            "00000000-0000-0000-0000-000000000000",
        ] {
            assert!(is_canonical_uuid_text(text), "{text} should be canonical");
        }
        for text in [
            "{0f0e0d0c-0b0a-4908-8706-050403020100}",
            "urn:uuid:0f0e0d0c-0b0a-4908-8706-050403020100",
            "0f0e0d0c0b0a49088706050403020100",
            " 0f0e0d0c-0b0a-4908-8706-050403020100",
            "0f0e0d0c-0b0a-4908-8706-050403020100\n",
            "0f0e0d0c-0b0a-4908-8706-05040302010\u{0661}",
            "0f0e0d0c_0b0a_4908_8706_050403020100",
        ] {
            assert!(!is_canonical_uuid_text(text), "{text:?} should be refused");
        }
    }

    #[test]
    fn admission_normalizes_to_lowercase() {
        let admitted = admit_uuid("0F0E0D0C-0B0A-4908-8706-050403020100");
        assert_eq!(
            admitted.map(|id| id.to_string()).ok().as_deref(),
            Some("0f0e0d0c-0b0a-4908-8706-050403020100")
        );
    }

    #[test]
    fn absent_and_malformed_values_are_distinct_refusals() {
        assert_eq!(admit_optional_uuid(None), Err(UuidRefusal::Null));
        assert_eq!(
            admit_optional_uuid(Some("{0f0e0d0c-0b0a-4908-8706-050403020100}")),
            Err(UuidRefusal::Malformed)
        );
        assert_eq!(admit_uuid(""), Err(UuidRefusal::Malformed));
    }

    #[test]
    fn each_boundary_maps_the_refusal_to_its_own_envelope() {
        let validation = UuidRefusal::Null.validation("Invalid job request", "jobId");
        assert_eq!(
            validation,
            AppError::from(crate::errors::validation_error(
                "Invalid job request",
                Some("jobId"),
                Some("must not be null"),
            ))
        );
        assert_eq!(
            UuidRefusal::Malformed.validation("Invalid job request", "jobId"),
            AppError::from(crate::errors::validation_error(
                "Invalid job request",
                Some("jobId"),
                Some(INVALID_UUID_REASON),
            ))
        );
        assert_eq!(
            UuidRefusal::Null.domain_value("jobId"),
            AppError::DomainValue("jobId must not be null".to_owned())
        );
        assert_eq!(
            UuidRefusal::Malformed.domain_value("jobId"),
            AppError::DomainValue("jobId must be a valid UUID".to_owned())
        );
    }

    #[test]
    fn name_uuid_sets_version_three_and_rfc_variant() {
        let id = name_uuid("deterministic-v1|a|b|c|d");
        assert_eq!(id.get_version_num(), 3);
        assert_eq!(id.get_variant(), uuid::Variant::RFC4122);
        assert_eq!(id, name_uuid("deterministic-v1|a|b|c|d"));
        assert_ne!(id, name_uuid("deterministic-v1|a|b|c|e"));
    }
}
