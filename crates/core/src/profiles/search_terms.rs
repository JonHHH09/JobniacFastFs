//! Search postings of a profile aggregate.
//!
//! The field keys, weights and collection order are fixed, because the search index and the
//! matched-field ordering of `profile_search` depend on them. Postings come from
//! `jobniac_text::search::SearchTerm::from_text`, so tokenization is the shared one.

use jobniac_text::search::SearchTerm;
use uuid::Uuid;

use super::model::ProfileGraph;

/// Builds every posting of the aggregate, in collection order.
///
/// PostgreSQL writers replace the profile's stored postings inside the same transaction as the
/// graph write, and the list uses this function for the exact keys and weights.
pub fn profile_search_terms(graph: &ProfileGraph) -> Vec<SearchTerm> {
    let profile = &graph.profile;
    let profile_id = profile.id;
    let mut terms = Vec::new();
    let mut add = |field_key: String, text: Option<&str>, weight: i64| {
        terms.extend(SearchTerm::from_text(profile_id, &field_key, text, weight));
    };
    add("profile.fullName".to_owned(), Some(&profile.full_name), 8);
    add("profile.email".to_owned(), Some(&profile.email), 5);
    add("profile.summary".to_owned(), profile.summary.as_deref(), 3);
    for value in &graph.contacts {
        let id = value.id;
        add(key(id, "contacts", "type"), Some(&value.contact_type), 2);
        add(key(id, "contacts", "value"), Some(&value.contact_value), 2);
        add(key(id, "contacts", "label"), value.label.as_deref(), 1);
    }
    for value in &graph.links {
        let id = value.id;
        add(key(id, "links", "type"), Some(&value.link_type), 3);
        add(key(id, "links", "url"), Some(&value.url), 3);
        add(key(id, "links", "label"), value.label.as_deref(), 2);
    }
    for value in &graph.skills {
        let id = value.id;
        add(key(id, "skills", "skill"), Some(&value.skill), 7);
        add(
            key(id, "skills", "normalized"),
            Some(&value.normalized_skill),
            7,
        );
        add(key(id, "skills", "category"), value.category.as_deref(), 3);
    }
    for value in &graph.languages {
        let id = value.id;
        add(key(id, "languages", "language"), Some(&value.language), 4);
        add(
            key(id, "languages", "normalized"),
            Some(&value.normalized_language),
            4,
        );
        add(
            key(id, "languages", "proficiency"),
            value.proficiency.as_deref(),
            2,
        );
    }
    for value in &graph.education {
        let id = value.id;
        add(
            key(id, "education", "institution"),
            value.institution.as_deref(),
            4,
        );
        add(key(id, "education", "degree"), value.degree.as_deref(), 3);
        add(key(id, "education", "field"), value.field.as_deref(), 4);
        add(
            key(id, "education", "location"),
            value.location.as_deref(),
            2,
        );
        add(
            key(id, "education", "focus"),
            value.relevant_focus.as_deref(),
            3,
        );
    }
    for value in &graph.experiences {
        let id = value.id;
        add(
            key(id, "experience", "company"),
            value.company.as_deref(),
            4,
        );
        add(key(id, "experience", "title"), value.title.as_deref(), 6);
        add(
            key(id, "experience", "location"),
            value.location.as_deref(),
            2,
        );
        add(
            key(id, "experience", "description"),
            value.description.as_deref(),
            3,
        );
    }
    for value in &graph.projects {
        let id = value.id;
        add(key(id, "projects", "name"), value.name.as_deref(), 5);
        add(key(id, "projects", "url"), value.url.as_deref(), 3);
        add(
            key(id, "projects", "description"),
            value.description.as_deref(),
            3,
        );
    }
    for value in &graph.project_technologies {
        let id = value.id;
        add(
            key(id, "technologies", "technology"),
            Some(&value.technology),
            5,
        );
        add(
            key(id, "technologies", "normalized"),
            Some(&value.normalized_technology),
            5,
        );
    }
    terms
}

/// A child field key such as `skills:<id>:normalized`.
fn key(id: Uuid, prefix: &str, field: &str) -> String {
    format!("{prefix}:{id}:{field}")
}
