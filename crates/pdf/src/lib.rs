//! Deterministic Standard-14 PDF layout and writing for Jobniac.
//!
//! Layout is pure: it turns already-validated text into fixed rows and pages.
//! Rendering writes unembedded Helvetica and Helvetica-Bold with WinAnsi encoding through
//! `pdf-writer`; [`readback`] repeats the structural checks with `lopdf`. No IO happens here.
#![forbid(unsafe_code)]

pub mod error;
pub mod layout;
pub mod metrics;
pub mod readback;
pub mod render;

pub use error::PdfError;
