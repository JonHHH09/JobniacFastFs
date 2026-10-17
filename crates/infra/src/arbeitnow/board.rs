//! Parent-side admission of the worker's raw Arbeitnow answer: validate the transport result, admit
//! it, then parse it.
//!
//! The child already moved the bytes; this module only checks that the answer is acceptable (status
//! range, empty body for a non-200 status, body bound), runs the core admission on the status and
//! headers, and for a 200 parses the body into a [`Page`]. The wall-clock instant for the
//! `Retry-After` date form is passed in, so the mapping is pure.

use jobniac_core::arbeitnow::board::{
    MAX_BODY_BYTES, Page, admit_response, parse_page, validate_request,
};
use jobniac_core::arbeitnow::failure::BoardFailure;
use jobniac_core::arbeitnow::instant::ArbeitnowInstant;

use crate::workers::fetch::board::{
    BoardCategory, BoardFailure as TransportFailure, BoardResponse,
};

/// Maps a failure of the worker transport to the core failure, keeping its category and
/// retryability closed.
#[must_use]
pub fn map_transport_failure(failure: TransportFailure) -> BoardFailure {
    match failure.category {
        BoardCategory::Transport => BoardFailure::Transport,
        BoardCategory::BodySize => BoardFailure::BodySize,
        BoardCategory::HttpStatus => BoardFailure::HttpStatus {
            retryable: failure.retryable,
        },
    }
}

/// Admits the raw answer and maps a 200 body to a page.
///
/// A response outside status `100..=599`, or a non-200 answer that carries a body, is a retryable
/// transport failure (the parent refuses what the child never produces). The core admission then
/// classifies the status and headers (429 with its bounded `Retry-After`, other statuses as
/// `http_status`, the media type and the declared length) before the body is looked at, and the
/// body is bounded again before parsing.
///
/// # Errors
///
/// The classified [`BoardFailure`].
pub fn admit_board_response(
    response: BoardResponse,
    requested_page: u32,
    visa: Option<bool>,
    now: ArbeitnowInstant,
) -> Result<Page, BoardFailure> {
    validate_request(i64::from(requested_page), visa)?;
    if !(100..=599).contains(&response.status) {
        return Err(BoardFailure::Transport);
    }
    if response.status != 200 && !response.body.is_empty() {
        return Err(BoardFailure::Transport);
    }
    let declared = admit_response(response.status, &response.headers, now)?;
    if response.body.len() > MAX_BODY_BYTES {
        return Err(BoardFailure::BodySize);
    }
    if declared.is_some_and(|length| length != response.body.len() as u64) {
        return Err(BoardFailure::Transport);
    }
    parse_page(&response.body, requested_page, visa)
}

/// The same mapping over the result of the worker call: a transport failure is mapped, a response
/// is admitted by [`admit_board_response`].
///
/// # Errors
///
/// The classified [`BoardFailure`].
pub fn admit_board_result(
    result: Result<BoardResponse, TransportFailure>,
    requested_page: u32,
    visa: Option<bool>,
    now: ArbeitnowInstant,
) -> Result<Page, BoardFailure> {
    match result {
        Ok(response) => admit_board_response(response, requested_page, visa, now),
        Err(failure) => Err(map_transport_failure(failure)),
    }
}
