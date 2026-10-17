//! Fail-closed stand-in for targets without a qualified generated-asset namespace (macOS is
//! unqualified). Nothing here touches the filesystem: every operation reports `Unsupported`, and
//! [`supported`] is `false` so the protocol refuses before any I/O.

use super::{Access, Publish, PublishError};
use std::ffi::OsString;
use std::fs::File;
use std::io;
use uuid::Uuid;

/// Identity placeholder; never produced on this target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Stamp;

fn unsupported() -> io::Error {
    io::Error::from(io::ErrorKind::Unsupported)
}

/// Publication and namespace control are not available on this target.
pub(crate) fn supported() -> bool {
    false
}

/// Identity of an opened object; unavailable on this target.
pub(crate) fn stamp_of(_file: &File) -> io::Result<Stamp> {
    Err(unsupported())
}

/// Directory creation with a private mode; unavailable on this target.
pub(crate) fn create_private_directory(_path: &str) -> io::Result<()> {
    Err(unsupported())
}

/// Flush of an existing object; unavailable on this target.
pub(crate) fn sync_existing(_file: &File) -> io::Result<()> {
    Err(unsupported())
}

/// Capability content; unavailable on this target.
pub(crate) fn capability_bytes(_root: &Root, _root_id: Uuid) -> io::Result<Vec<u8>> {
    Err(unsupported())
}

/// The pinned root; never constructed on this target.
#[derive(Debug)]
pub(crate) struct Root(());

impl Root {
    pub(crate) fn open(_path: &str) -> io::Result<Self> {
        Err(unsupported())
    }

    pub(crate) fn list_names(&self) -> io::Result<Vec<OsString>> {
        Err(unsupported())
    }

    pub(crate) fn verify(&self) -> io::Result<()> {
        Err(unsupported())
    }

    pub(crate) fn qualifies_for_controls(&self) -> io::Result<bool> {
        Err(unsupported())
    }

    pub(crate) fn require_capability_link(&self) -> io::Result<()> {
        Err(unsupported())
    }

    pub(crate) fn open_child(&self, _name: &str, _access: Access) -> io::Result<File> {
        Err(unsupported())
    }

    pub(crate) fn create_child_with(&self, _name: &str, _data: &[u8]) -> io::Result<()> {
        Err(unsupported())
    }

    pub(crate) fn discard_child(&self, _name: &str) -> io::Result<()> {
        Err(unsupported())
    }

    pub(crate) fn probe_publication(&self) -> Result<(), PublishError> {
        Err(PublishError::Unsupported)
    }

    pub(crate) fn publish_anonymous(
        &self,
        _name: &str,
        _content: &[u8],
    ) -> Result<Publish, PublishError> {
        Err(PublishError::Unsupported)
    }

    pub(crate) fn unlink(&self, _name: &str, _file: &File) -> io::Result<()> {
        Err(unsupported())
    }

    pub(crate) fn sync(&self) -> io::Result<()> {
        Err(unsupported())
    }

    pub(crate) fn absent(&self, _name: &str) -> io::Result<bool> {
        Err(unsupported())
    }

    pub(crate) fn name_refers_to(&self, _name: &str, _file: &File) -> io::Result<bool> {
        Err(unsupported())
    }

    pub(crate) fn lock_exclusive(&self, _file: &File) -> io::Result<()> {
        Err(unsupported())
    }
}
