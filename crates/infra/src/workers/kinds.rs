//! Worker kinds and their limit profiles, including the rendering limits of
//! the `pdf` worker kind.
//!
//! Each kind is one hidden `jobniac-cli __worker <name>` mode. The limits are compiled in: the
//! child applies its own limits from this table before it reads its request, and the supervisor
//! enforces the same table from the parent side.

use std::io::Write;
use std::time::Duration;

use jobniac_core::job_links::urls::MAX_URL_BYTES;

use crate::workers::child::WorkerFault;
use crate::workers::fetch;
use crate::workers::pdf::{MAX_FILE_BYTES, MAX_PDFIUM_DIR_BYTES};
use crate::workers::probe;
use crate::workers::sandbox::SandboxProfile;

/// The largest response of the link fetch child (1 MiB). A larger
/// encoded response is replaced by the generic failure.
pub const FETCH_LINK_IPC_BYTES: u64 = 1024 * 1024;

/// The largest response of the Arbeitnow child: the decoded body (4 MiB) plus 128 KiB for the status, the header block and the framing.
pub const FETCH_ARBEITNOW_IPC_BYTES: u64 = 4 * 1024 * 1024 + 128 * 1024;

/// The bytes of one mebibyte.
const MIB: u64 = 1024 * 1024;

/// Pixels per side of a rendered page.
const RENDER_MAX_DIMENSION: u64 = 1600;

/// Largest rendered response of the page-rendering operation, plus the frame prefix: four bytes per
/// pixel, 64 KiB of headroom, 12 bytes of framing and four bytes for the frame prefix.
const RENDER_OUTPUT_BYTES: u64 = RENDER_MAX_DIMENSION * RENDER_MAX_DIMENSION * 4 + 65_536 + 12 + 4;

/// Largest extraction request body: the operation byte, the two-byte directory length, the largest
/// PDFium directory, the largest PDF content (the import limit) and one byte more, so an
/// oversized document reaches `validate_pdf_content` and is refused with the size message.
/// Both the parent and the child check the content size; the transport limit only bounds the read.
const EXTRACT_REQUEST_BYTES: usize = 1 + 2 + MAX_PDFIUM_DIR_BYTES + MAX_FILE_BYTES + 1;

/// Signature of a child operation. It reads the request body and writes its response frame (or
/// other bytes) to `output`. An error makes the child exit with status 1 and no diagnostics.
pub type WorkerHandler = fn(&[u8], &mut dyn Write) -> Result<(), WorkerFault>;

/// The resource limits of one worker operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerLimits {
    /// Address space on Linux (`RLIMIT_AS`); committed process memory and job memory on Windows.
    pub memory_bytes: u64,
    /// CPU seconds: `RLIMIT_CPU` on Linux; per-process user time on Windows.
    pub cpu_seconds: u64,
    /// Wall-clock time from the start of the operation, enforced by the parent.
    pub wall: Duration,
    /// Total bytes the child may write to stdout, the frame prefix included.
    pub output_bytes: u64,
    /// Open file descriptors (`RLIMIT_NOFILE` on Linux; not applied on Windows).
    pub open_files: u64,
}

/// One worker kind, as named on the command line of the hidden `__worker` mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WorkerKind {
    /// PDF text extraction: 512 MiB, 10 CPU seconds, 15 s wall, 16 MiB of output.
    PdfExtract,
    /// PDF page rendering to PNG: 1 GiB address space, 10 CPU seconds, 15 s wall, 1600 px pages.
    PdfRender,
    /// Test-only probe whose operations deliberately exceed each limit. It is compiled in but
    /// refused unless the internal marker is set.
    Probe,
    /// One guarded GET of a job page: public addresses only, no redirects.
    FetchLink,
    /// The fixed-host GET of the Arbeitnow board API.
    FetchArbeitnow,
}

impl WorkerKind {
    /// Every worker kind.
    pub const ALL: [Self; 5] = [
        Self::PdfExtract,
        Self::PdfRender,
        Self::Probe,
        Self::FetchLink,
        Self::FetchArbeitnow,
    ];

    /// The name used on the command line, for example `pdf-extract`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::PdfExtract => "pdf-extract",
            Self::PdfRender => "pdf-render",
            Self::Probe => "probe",
            Self::FetchLink => "fetch-link",
            Self::FetchArbeitnow => "fetch-arbeitnow",
        }
    }

    /// The kind with the given command-line name.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == name)
    }

    /// The limit profile of the kind.
    #[must_use]
    pub const fn limits(self) -> WorkerLimits {
        match self {
            Self::PdfExtract => WorkerLimits {
                memory_bytes: 512 * MIB,
                cpu_seconds: 10,
                wall: Duration::from_secs(15),
                output_bytes: 16 * MIB,
                open_files: 64,
            },
            Self::PdfRender => WorkerLimits {
                memory_bytes: 1024 * MIB,
                cpu_seconds: 10,
                wall: Duration::from_secs(15),
                output_bytes: RENDER_OUTPUT_BYTES,
                open_files: 64,
            },
            Self::Probe => WorkerLimits {
                memory_bytes: 256 * MIB,
                cpu_seconds: 2,
                // Longer than the CPU limit under test load, so the CPU limit is the one that fires.
                wall: Duration::from_secs(10),
                output_bytes: 64 * 1024,
                open_files: 64,
            },
            // RLIMIT_AS 1 GiB, RLIMIT_CPU 5, 20 s wall, and the 1 MiB IPC cap, plus the four-byte
            // frame prefix.
            Self::FetchLink => WorkerLimits {
                memory_bytes: 1024 * MIB,
                cpu_seconds: 5,
                wall: Duration::from_secs(20),
                output_bytes: FETCH_LINK_IPC_BYTES + 4,
                open_files: 64,
            },
            // The same memory and CPU limits, 10 s wall, and the IPC cap of the decoded body (4 MiB)
            // plus the header block and framing.
            Self::FetchArbeitnow => WorkerLimits {
                memory_bytes: 1024 * MIB,
                cpu_seconds: 5,
                wall: Duration::from_secs(10),
                output_bytes: FETCH_ARBEITNOW_IPC_BYTES + 4,
                open_files: 64,
            },
        }
    }

    /// The largest request body the kind accepts, in bytes. The child refuses a larger frame
    /// before it reads the body.
    #[must_use]
    pub const fn request_limit(self) -> usize {
        match self {
            Self::PdfExtract => EXTRACT_REQUEST_BYTES,
            Self::PdfRender => 2 * 1024 * 1024,
            Self::Probe => 64 * 1024,
            // The URL (8192 bytes at most) and the link fetch request has no other fields.
            Self::FetchLink => MAX_URL_BYTES + 1,
            // The page number (four bytes) and the visa tri-state (one byte).
            Self::FetchArbeitnow => 5,
        }
    }

    /// The confinement profile the operation of this kind applies to itself, or `None` for the
    /// test-only probe. The match is exhaustive, so a new kind must decide; the child refuses to
    /// return a response of a kind with a profile unless the confinement is in force
    /// ([`crate::workers::child::serve`]).
    #[must_use]
    pub const fn sandbox_profile(self) -> Option<SandboxProfile> {
        match self {
            Self::PdfExtract => Some(SandboxProfile::PdfExtract),
            Self::PdfRender => Some(SandboxProfile::PdfRender),
            Self::FetchLink | Self::FetchArbeitnow => Some(SandboxProfile::Fetch),
            Self::Probe => None,
        }
    }

    /// The number of children of this kind that may run at the same time.
    #[must_use]
    pub const fn max_concurrent(self) -> usize {
        match self {
            Self::PdfExtract
            | Self::PdfRender
            | Self::Probe
            | Self::FetchLink
            | Self::FetchArbeitnow => 2,
        }
    }

    /// The value the internal marker must carry for this kind.
    #[must_use]
    pub fn marker_value(self) -> String {
        format!("{}-v1", self.name())
    }

    /// The operation of this kind, or `None` when the kind has no operation in this build.
    #[must_use]
    pub fn handler(self) -> Option<WorkerHandler> {
        match self {
            Self::Probe => Some(probe::handle),
            Self::FetchLink => Some(fetch::link::handle),
            Self::FetchArbeitnow => Some(fetch::board::handle),
            Self::PdfExtract | Self::PdfRender => None,
        }
    }
}
