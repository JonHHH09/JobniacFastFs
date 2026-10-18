//! Explicit, idempotent first-time setup of the owned generated-artifact root.
//!
//! The setup initializes an empty root's private controls and registers the same identity in the
//! database. It never repairs, adopts, overwrites or deletes: a state it cannot prove is refused
//! with a fixed reason, and no path, identity or source text is ever part of a refusal. The file
//! and registry operations sit behind ports; the composition root supplies the filesystem and
//! SQLite adapters.

use std::sync::Arc;

use async_trait::async_trait;
use uuid::Uuid;

use crate::errors::AppError;

/// What the root directory holds before setup decides anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupDirectoryState {
    /// Missing, not a plain directory, or unreadable.
    Unusable,
    /// A plain directory with no entries.
    Empty,
    /// Holds a well-formed owned marker (not yet proven).
    Marked,
    /// Non-empty without a well-formed owned marker.
    Foreign,
}

/// The outcome word of a successful setup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupStatus {
    /// The root was initialized and registered.
    Created,
    /// A proven root was registered; nothing on disk changed.
    Registered,
    /// The root was already set up and registered.
    AlreadySetUp,
}

impl SetupStatus {
    /// The word printed after the root identity.
    #[must_use]
    pub const fn text(self) -> &'static str {
        match self {
            Self::Created => "created and registered",
            Self::Registered => "initialized root registered",
            Self::AlreadySetUp => "already set up",
        }
    }
}

/// The fixed, caller-safe reasons of a refused setup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupRefusal {
    /// The root is missing, unreadable or not a plain directory.
    RootUnusable,
    /// The root is non-empty and holds no owned marker.
    RootNotEmpty,
    /// The marker identity differs from the configured identity.
    IdentityMismatch,
    /// The controls of the root could not be proven.
    Unproven,
    /// The identity is registered to a different root.
    AlreadyRegistered,
    /// The registration could not be read or written.
    Database,
    /// Initialization or inspection failed.
    Failed,
}

impl SetupRefusal {
    /// The refusal text without the final period.
    #[must_use]
    pub const fn text(self) -> &'static str {
        match self {
            Self::RootUnusable => {
                "the generated root is missing, unreadable or not a plain directory"
            }
            Self::RootNotEmpty => "the generated root is not empty and holds no owned marker",
            Self::IdentityMismatch => {
                "the configured generated root identity does not match the root"
            }
            Self::Unproven => "the generated root controls could not be proven",
            Self::AlreadyRegistered => "the identity is already registered to a different root",
            Self::Database => "the generated root registration could not be read or written",
            Self::Failed => "the generated root could not be initialized",
        }
    }
}

/// The result of a successful setup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetupResult {
    /// What setup did.
    pub status: SetupStatus,
    /// The registered identity of the root.
    pub root_id: Uuid,
}

/// A filesystem failure of the setup ports. It carries no text: the use case maps it to a
/// refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetupFileError;

/// Filesystem port of the setup. Methods never report paths.
pub trait SetupFiles: Send + Sync {
    /// Classifies the root directory without modifying it.
    fn inspect(&self) -> SetupDirectoryState;
    /// The unproven identity candidate of a well-formed marker, if any.
    fn read_marker_identity(&self) -> Option<Uuid>;
    /// Initializes the private controls of an empty root for `root_id`.
    ///
    /// # Errors
    ///
    /// Fails when the controls cannot be created.
    fn initialize(&self, root_id: Uuid) -> Result<(), SetupFileError>;
    /// Proves that the private marker, capability and lock controls belong to `root_id`.
    ///
    /// # Errors
    ///
    /// Fails unless the controls prove `root_id`.
    fn qualify(&self, root_id: Uuid) -> Result<(), SetupFileError>;
}

/// Registry port of the setup.
#[async_trait]
pub trait SetupRegistry: Send + Sync {
    /// Whether `root_id` is registered.
    async fn is_registered(&self, root_id: Uuid) -> Result<bool, AppError>;
    /// Inserts the identity if absent. Never updates or deletes a registration.
    async fn register(&self, root_id: Uuid) -> Result<(), AppError>;
}

/// The explicit setup of one owned root.
pub struct GeneratedRootSetup {
    files: Arc<dyn SetupFiles>,
    registry: Arc<dyn SetupRegistry>,
    configured_id: Option<Uuid>,
}

impl std::fmt::Debug for GeneratedRootSetup {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GeneratedRootSetup")
            .field("configured", &self.configured_id.is_some())
            .finish_non_exhaustive()
    }
}

impl GeneratedRootSetup {
    /// Builds the setup over its ports. `configured_id` is the identity from
    /// `JOBNIAC_GENERATED_ROOT_ID`, when it is set.
    #[must_use]
    pub fn new(
        files: Arc<dyn SetupFiles>,
        registry: Arc<dyn SetupRegistry>,
        configured_id: Option<Uuid>,
    ) -> Self {
        Self {
            files,
            registry,
            configured_id,
        }
    }

    /// Runs the setup once.
    ///
    /// # Errors
    ///
    /// The fixed refusal of the first step that cannot be proven. Nothing is repaired.
    pub async fn run(&self) -> Result<SetupResult, SetupRefusal> {
        match self.files.inspect() {
            SetupDirectoryState::Unusable => Err(SetupRefusal::RootUnusable),
            SetupDirectoryState::Foreign => Err(SetupRefusal::RootNotEmpty),
            SetupDirectoryState::Empty => self.create().await,
            SetupDirectoryState::Marked => self.confirm().await,
        }
    }

    /// Initializes, qualifies and registers an empty root.
    async fn create(&self) -> Result<SetupResult, SetupRefusal> {
        let root_id = self.configured_id.unwrap_or_else(Uuid::new_v4);
        // A registration without its directory controls is a mismatch, never adopted.
        if self.is_registered(root_id).await? {
            return Err(SetupRefusal::AlreadyRegistered);
        }
        self.files
            .initialize(root_id)
            .map_err(|_| SetupRefusal::Failed)?;
        self.qualify(root_id)?;
        self.register(root_id).await?;
        Ok(SetupResult {
            status: SetupStatus::Created,
            root_id,
        })
    }

    /// Confirms a marked root and registers its proven identity.
    async fn confirm(&self) -> Result<SetupResult, SetupRefusal> {
        let root_id = self
            .files
            .read_marker_identity()
            .ok_or(SetupRefusal::Unproven)?;
        if self
            .configured_id
            .is_some_and(|configured| configured != root_id)
        {
            return Err(SetupRefusal::IdentityMismatch);
        }
        self.qualify(root_id)?;
        if self.is_registered(root_id).await? {
            return Ok(SetupResult {
                status: SetupStatus::AlreadySetUp,
                root_id,
            });
        }
        // Reached only after an earlier run initialized the controls but could not register them:
        // the proven identity is inserted, and nothing on disk is touched.
        self.register(root_id).await?;
        Ok(SetupResult {
            status: SetupStatus::Registered,
            root_id,
        })
    }

    fn qualify(&self, root_id: Uuid) -> Result<(), SetupRefusal> {
        self.files
            .qualify(root_id)
            .map_err(|_| SetupRefusal::Unproven)
    }

    async fn is_registered(&self, root_id: Uuid) -> Result<bool, SetupRefusal> {
        self.registry
            .is_registered(root_id)
            .await
            .map_err(|_| SetupRefusal::Database)
    }

    async fn register(&self, root_id: Uuid) -> Result<(), SetupRefusal> {
        self.registry
            .register(root_id)
            .await
            .map_err(|_| SetupRefusal::Database)
    }
}
