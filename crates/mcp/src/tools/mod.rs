//! The tool modules of the server and the composition of their handlers.
//!
//! One module per capability area. A module exposes a handler type and a `register`
//! function; [`default_registry`] composes the default services. A new module needs one `register` line here and its catalog entries in
//! `tools.json`. Opt-in tools are composed only when their service exists.

pub mod arbeitnow;
pub mod artifacts;
pub mod deletion;
pub mod documents;
pub mod generation;
pub mod health;
pub mod job_links;
pub mod jobs;
pub mod matching;
pub mod profile_pdf;
pub mod profiles;
pub mod reviews;

use std::sync::Arc;

use crate::catalog::Catalog;
use crate::registry::ToolRegistry;
use crate::server::{McpServeError, McpServices};

/// The registry of the tools whose services are composed in `services`.
pub fn default_registry(services: &McpServices) -> Result<ToolRegistry, McpServeError> {
    let mut registry = ToolRegistry::new(Catalog::load()?);
    registry.register(
        health::NAME,
        health::HealthTool::new(Arc::clone(&services.health)),
    )?;
    profiles::register(
        &mut registry,
        services.profile_list.clone(),
        services.profile_details.clone(),
        services.profile_search.clone(),
    )?;
    jobs::register(
        &mut registry,
        services.job_service.clone(),
        services.job_search.clone(),
    )?;
    matching::register(&mut registry, services.matching_service.clone())?;
    reviews::register(&mut registry, services.review_service.clone())?;
    documents::register(&mut registry, services.document_service.clone())?;
    profile_pdf::register(&mut registry, services.profile_pdf_service.clone())?;
    generation::register(&mut registry, services.generation_service.clone())?;
    artifacts::register(&mut registry, services.document_artifact_service.clone())?;
    deletion::register(&mut registry, services.deletion.clone())?;
    job_links::register(&mut registry, services.job_link_service.clone())?;
    arbeitnow::register(&mut registry, services.arbeitnow.clone())?;
    Ok(registry)
}
