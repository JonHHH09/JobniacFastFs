//! Generation use cases: resume content preparation and French translation.
//!
//! This module holds the pure preparation of resume text: the master, Canadian English and
//! Canadian French bodies ([`content`]) and the offline French translator with its glossary
//! ([`translation`]). Nothing here does IO. The glossary file is read by the infrastructure layer
//! and handed over as text.

pub mod content;
pub mod facade;
pub mod generic;
pub mod standalone;
pub mod translation;
