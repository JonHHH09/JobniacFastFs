//! Binary frames of the fetch workers.
//!
//! The request and response bodies travel inside the worker frame of
//! [`crate::workers::protocol`]; this module only defines their fields. Every decoder is strict:
//! an unknown tag, a short field or trailing bytes is a malformed response.
//!
//! Link response: `0x01 status:u16 title description` or `0x00 failure:u8`, where a text field is
//! `0x00` (absent) or `0x01 len:u32 utf8`.
//!
//! Board request: `page:u32 visa:u8` (`0` absent, `1` false, `2` true).
//!
//! Board response: `0x01 status:u16 count:u16 (name_len:u16 name value_len:u32 value)* body_len:u32
//! body` or `0x00 category:u8 retryable:u8`.

use jobniac_core::job_links::{JobLinkFetchFailure, JobLinkFetchResult, html::MAX_BODY_BYTES};

use super::board::{BoardCategory, BoardFailure, BoardResponse};

const TAG_OK: u8 = 0x01;
const TAG_FAILED: u8 = 0x00;

/// Largest text field of a link response: three bytes for each byte of the body, which bounds a
/// decoded title or description.
const MAX_TEXT_BYTES: usize = MAX_BODY_BYTES as usize * 3;

/// Largest raw header block of a board response.
const MAX_BOARD_HEADER_BYTES: usize = 16 * 1024;

/// The code of a link failure on the wire.
#[must_use]
pub const fn failure_code(failure: JobLinkFetchFailure) -> u8 {
    match failure {
        JobLinkFetchFailure::Unavailable => 0,
        JobLinkFetchFailure::Interrupted => 1,
        JobLinkFetchFailure::InvalidUrl => 2,
        JobLinkFetchFailure::RedirectNotFollowed => 3,
        JobLinkFetchFailure::UnsafeAddress => 4,
        JobLinkFetchFailure::IpLiteralRequired => 5,
    }
}

/// The failure of a wire code, or `Unavailable` for an unknown one.
#[must_use]
pub const fn failure_from_code(code: u8) -> JobLinkFetchFailure {
    match code {
        1 => JobLinkFetchFailure::Interrupted,
        2 => JobLinkFetchFailure::InvalidUrl,
        3 => JobLinkFetchFailure::RedirectNotFollowed,
        4 => JobLinkFetchFailure::UnsafeAddress,
        5 => JobLinkFetchFailure::IpLiteralRequired,
        _ => JobLinkFetchFailure::Unavailable,
    }
}

/// Encodes a link outcome. A success whose encoding exceeds `limit` becomes the generic failure.
#[must_use]
pub fn encode_link(
    outcome: &Result<JobLinkFetchResult, JobLinkFetchFailure>,
    limit: usize,
) -> Vec<u8> {
    let encoded = match outcome {
        Ok(result) => {
            let mut out = vec![TAG_OK];
            out.extend_from_slice(&u16::try_from(result.http_status).unwrap_or(0).to_be_bytes());
            push_text(&mut out, result.title.as_deref());
            push_text(&mut out, result.description.as_deref());
            out
        }
        Err(failure) => vec![TAG_FAILED, failure_code(*failure)],
    };
    if encoded.len() > limit {
        return vec![TAG_FAILED, failure_code(JobLinkFetchFailure::Unavailable)];
    }
    encoded
}

fn push_text(out: &mut Vec<u8>, text: Option<&str>) {
    match text {
        None => out.push(0),
        Some(text) => {
            out.push(1);
            out.extend_from_slice(&u32::try_from(text.len()).unwrap_or(u32::MAX).to_be_bytes());
            out.extend_from_slice(text.as_bytes());
        }
    }
}

/// A frame that does not follow the layout of this module. It carries no detail by design.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MalformedFrame;

/// Decodes a link response. A malformed frame is [`MalformedFrame`], which the parent reports as
/// the generic failure.
///
/// # Errors
///
/// Returns [`MalformedFrame`] for any malformed frame.
pub fn decode_link(
    body: &[u8],
) -> Result<Result<JobLinkFetchResult, JobLinkFetchFailure>, MalformedFrame> {
    decode_link_frame(body).map_err(|()| MalformedFrame)
}

fn decode_link_frame(body: &[u8]) -> Result<Result<JobLinkFetchResult, JobLinkFetchFailure>, ()> {
    let mut reader = Reader::new(body);
    match reader.byte()? {
        TAG_FAILED => {
            let failure = failure_from_code(reader.byte()?);
            reader.finish()?;
            Ok(Err(failure))
        }
        TAG_OK => {
            let status = i64::from(reader.u16()?);
            let title = reader.text()?;
            let description = reader.text()?;
            reader.finish()?;
            if !(100..=599).contains(&status) {
                return Err(());
            }
            Ok(Ok(JobLinkFetchResult {
                title,
                description,
                http_status: status,
            }))
        }
        _ => Err(()),
    }
}

/// Encodes a board request.
#[must_use]
pub fn encode_board_request(page: u32, visa: Option<bool>) -> [u8; 5] {
    let visa_byte = match visa {
        None => 0,
        Some(false) => 1,
        Some(true) => 2,
    };
    let page = page.to_be_bytes();
    [page[0], page[1], page[2], page[3], visa_byte]
}

/// Decodes a board request, or `None` for any other length or visa byte.
#[must_use]
pub fn decode_board_request(body: &[u8]) -> Option<(u32, Option<bool>)> {
    let [a, b, c, d, visa] = *body else {
        return None;
    };
    let visa = match visa {
        0 => None,
        1 => Some(false),
        2 => Some(true),
        _ => return None,
    };
    Some((u32::from_be_bytes([a, b, c, d]), visa))
}

/// The category of a board failure on the wire.
const fn category_code(category: BoardCategory) -> u8 {
    match category {
        BoardCategory::Transport => 0,
        BoardCategory::BodySize => 1,
        BoardCategory::HttpStatus => 2,
    }
}

/// Encodes a board outcome. A success larger than `limit` becomes a transport failure.
#[must_use]
pub fn encode_board(outcome: &Result<BoardResponse, BoardFailure>, limit: usize) -> Vec<u8> {
    let encoded = match outcome {
        Err(failure) => vec![
            TAG_FAILED,
            category_code(failure.category),
            u8::from(failure.retryable),
        ],
        Ok(response) => {
            let mut out = vec![TAG_OK];
            out.extend_from_slice(&response.status.to_be_bytes());
            out.extend_from_slice(
                &u16::try_from(response.headers.len())
                    .unwrap_or(u16::MAX)
                    .to_be_bytes(),
            );
            for (name, value) in &response.headers {
                out.extend_from_slice(&u16::try_from(name.len()).unwrap_or(u16::MAX).to_be_bytes());
                out.extend_from_slice(name);
                out.extend_from_slice(
                    &u32::try_from(value.len()).unwrap_or(u32::MAX).to_be_bytes(),
                );
                out.extend_from_slice(value);
            }
            out.extend_from_slice(
                &u32::try_from(response.body.len())
                    .unwrap_or(u32::MAX)
                    .to_be_bytes(),
            );
            out.extend_from_slice(&response.body);
            out
        }
    };
    if encoded.len() > limit {
        return vec![TAG_FAILED, category_code(BoardCategory::Transport), 1];
    }
    encoded
}

/// Decodes a board response. Header and body bounds are checked here as well, so a corrupted
/// frame cannot exceed them.
///
/// # Errors
///
/// Returns [`MalformedFrame`] for any malformed frame; the caller reports a transport failure.
pub fn decode_board(body: &[u8]) -> Result<Result<BoardResponse, BoardFailure>, MalformedFrame> {
    decode_board_frame(body).map_err(|()| MalformedFrame)
}

fn decode_board_frame(body: &[u8]) -> Result<Result<BoardResponse, BoardFailure>, ()> {
    let mut reader = Reader::new(body);
    match reader.byte()? {
        TAG_FAILED => {
            let category = match reader.byte()? {
                0 => BoardCategory::Transport,
                1 => BoardCategory::BodySize,
                2 => BoardCategory::HttpStatus,
                _ => return Err(()),
            };
            let retryable = match reader.byte()? {
                0 => false,
                1 => true,
                _ => return Err(()),
            };
            reader.finish()?;
            Ok(Err(BoardFailure {
                category,
                retryable,
            }))
        }
        TAG_OK => {
            let status = reader.u16()?;
            let count = usize::from(reader.u16()?);
            let mut headers = Vec::with_capacity(count);
            let mut header_bytes = 0_usize;
            for _ in 0..count {
                let name_length = usize::from(reader.u16()?);
                let name = reader.take(name_length)?.to_vec();
                let value_length = usize::try_from(reader.u32()?).map_err(|_| ())?;
                let value = reader.take(value_length)?.to_vec();
                header_bytes = header_bytes.saturating_add(name.len() + value.len() + 4);
                if header_bytes > MAX_BOARD_HEADER_BYTES {
                    return Err(());
                }
                headers.push((name, value));
            }
            let body_length = usize::try_from(reader.u32()?).map_err(|_| ())?;
            let body = reader.take(body_length)?.to_vec();
            reader.finish()?;
            Ok(Ok(BoardResponse {
                status,
                headers,
                body,
            }))
        }
        _ => Err(()),
    }
}

/// A bounds-checked cursor over a frame body.
struct Reader<'a> {
    rest: &'a [u8],
}

impl<'a> Reader<'a> {
    const fn new(rest: &'a [u8]) -> Self {
        Self { rest }
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], ()> {
        let (head, tail) = self.rest.split_at_checked(count).ok_or(())?;
        self.rest = tail;
        Ok(head)
    }

    fn byte(&mut self) -> Result<u8, ()> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, ()> {
        let bytes = self.take(2)?;
        Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
    }

    fn u32(&mut self) -> Result<u32, ()> {
        let bytes = self.take(4)?;
        Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn text(&mut self) -> Result<Option<String>, ()> {
        match self.byte()? {
            0 => Ok(None),
            1 => {
                let length = usize::try_from(self.u32()?).map_err(|_| ())?;
                if length > MAX_TEXT_BYTES {
                    return Err(());
                }
                let text = std::str::from_utf8(self.take(length)?).map_err(|_| ())?;
                Ok(Some(text.to_owned()))
            }
            _ => Err(()),
        }
    }

    fn finish(self) -> Result<(), ()> {
        if self.rest.is_empty() {
            Ok(())
        } else {
            Err(())
        }
    }
}
