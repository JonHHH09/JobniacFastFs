//! Filesystem operations of the explicit generated-root setup (`jobniac setup-generated-root`).
//!
//! Reads are best effort and only produce an *unproven candidate*: the identity in a well-formed
//! marker is never trusted here. Trust comes from [`OwnedAssetFiles::qualify_owned_root`] (the
//! exact marker bytes, the root-bound capability and the namespace lock) together with the database
//! registration in the setup use case. Nothing here repairs, adopts or removes anything except
//! through [`OwnedAssetFiles::initialize_owned_root`].

use crate::generated::asset_files::{OwnedAssetFiles, RemovalFailure};
use crate::generated::root::{ensure_private_directory, is_plain_directory};
use jobniac_core::generated_root::{MARKER_LIMIT, MARKER_NAME, parse_marker};
use jobniac_core::generated_root_setup::{SetupDirectoryState, SetupFileError, SetupFiles};
use std::fs::{self, File};
use std::io::Read;
use std::path::Path;
use uuid::Uuid;

/// What the root directory holds before setup decides anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootDirectoryState {
    /// Missing, not a plain directory, or unreadable.
    Unusable,
    /// A plain directory with no entries.
    Empty,
    /// Holds a well-formed owned marker (not yet proven).
    Marked,
    /// Non-empty without a well-formed owned marker.
    Foreign,
}

/// The filesystem port of setup for one root path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootSetupFiles {
    root: String,
    create_missing: bool,
}

impl RootSetupFiles {
    /// Binds setup to `root`. With `create_missing`, only the per-user default root may be created
    /// (privately); an explicit root is never created.
    #[must_use]
    pub fn new(root: impl Into<String>, create_missing: bool) -> Self {
        Self {
            root: root.into(),
            create_missing,
        }
    }

    /// Classifies the root directory without modifying it, except for the private creation of a
    /// missing default root.
    #[must_use]
    pub fn inspect(&self) -> RootDirectoryState {
        let path = Path::new(&self.root);
        if self.create_missing && fs::symlink_metadata(path).is_err() {
            let _ = ensure_private_directory(&self.root);
        }
        if !is_plain_directory(&self.root) {
            return RootDirectoryState::Unusable;
        }
        match fs::read_dir(path) {
            Ok(mut entries) => match entries.next() {
                None => RootDirectoryState::Empty,
                Some(Err(_)) => RootDirectoryState::Unusable,
                Some(Ok(_)) => {
                    if self.read_marker_identity().is_none() {
                        RootDirectoryState::Foreign
                    } else {
                        RootDirectoryState::Marked
                    }
                }
            },
            Err(_) => RootDirectoryState::Unusable,
        }
    }

    /// Parses the marker into an unproven identity candidate, or `None` when it is absent, not a
    /// regular file, larger than the limit or not well formed.
    #[must_use]
    pub fn read_marker_identity(&self) -> Option<Uuid> {
        let marker = Path::new(&self.root).join(MARKER_NAME);
        let info = fs::symlink_metadata(&marker).ok()?;
        if !info.is_file() || info.len() > MARKER_LIMIT as u64 {
            return None;
        }
        let mut data = Vec::new();
        File::open(&marker)
            .ok()?
            .take(MARKER_LIMIT as u64 + 1)
            .read_to_end(&mut data)
            .ok()?;
        parse_marker(&data)
    }

    /// Creates the controls of an empty root (see [`OwnedAssetFiles::initialize_owned_root`]).
    ///
    /// # Errors
    ///
    /// Fails with the namespace category when the controls cannot be created.
    pub fn initialize(&self, root_id: Uuid) -> Result<(), RemovalFailure> {
        OwnedAssetFiles::initialize_owned_root(&self.root, root_id)
    }

    /// Proves the controls of `root_id` under the namespace lock.
    ///
    /// # Errors
    ///
    /// Fails with the namespace category when the controls do not prove `root_id`.
    pub fn qualify(&self, root_id: Uuid) -> Result<(), RemovalFailure> {
        OwnedAssetFiles::new(Some(self.root.clone()), Some(root_id)).qualify_owned_root()
    }
}

impl SetupFiles for RootSetupFiles {
    fn inspect(&self) -> SetupDirectoryState {
        match RootSetupFiles::inspect(self) {
            RootDirectoryState::Unusable => SetupDirectoryState::Unusable,
            RootDirectoryState::Empty => SetupDirectoryState::Empty,
            RootDirectoryState::Marked => SetupDirectoryState::Marked,
            RootDirectoryState::Foreign => SetupDirectoryState::Foreign,
        }
    }

    fn read_marker_identity(&self) -> Option<Uuid> {
        RootSetupFiles::read_marker_identity(self)
    }

    fn initialize(&self, root_id: Uuid) -> Result<(), SetupFileError> {
        RootSetupFiles::initialize(self, root_id).map_err(|_| SetupFileError)
    }

    fn qualify(&self, root_id: Uuid) -> Result<(), SetupFileError> {
        RootSetupFiles::qualify(self, root_id).map_err(|_| SetupFileError)
    }
}
