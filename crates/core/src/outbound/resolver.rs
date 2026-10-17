//! One-shot pinned resolution.
//!
//! The core owns the policy, not the socket: the caller supplies an [`AddressLookup`] adapter (the
//! system resolver lives in the infrastructure layer). Each normalized `(host, port)` is looked up
//! once per resolver; the first validated answer is pinned and returned for later calls. A failed
//! lookup is not pinned.

use std::collections::BTreeMap;

use async_trait::async_trait;

use super::hostname::normalize_hostname;
use super::policy::{OutboundPolicyError, PolicyFailure, ResolverRecord, admit_records};

/// A failed lookup. It carries no cause, so nothing from the resolver reaches an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LookupFailed;

/// The port the resolver calls for raw TCP records of a normalized host name.
#[async_trait]
pub trait AddressLookup: Send + Sync {
    /// Returns the raw TCP records for `host` and `port`, or [`LookupFailed`].
    async fn lookup(&self, host: &str, port: i64) -> Result<Vec<ResolverRecord>, LookupFailed>;
}

/// Resolves each `(host, port)` once and remembers the validated answer.
#[derive(Debug, Default)]
pub struct PinnedResolver {
    pinned: BTreeMap<(String, i64), Vec<String>>,
}

impl PinnedResolver {
    /// An empty resolver with nothing pinned.
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolves `host` (normalized first) and returns the validated numeric addresses.
    pub async fn resolve(
        &mut self,
        lookup: &dyn AddressLookup,
        host: &str,
        port: i64,
    ) -> Result<Vec<String>, OutboundPolicyError> {
        let name = normalize_hostname(host)?;
        let key = (name, port);
        if let Some(pinned) = self.pinned.get(&key) {
            return Ok(pinned.clone());
        }
        let records = match lookup.lookup(&key.0, port).await {
            Ok(records) => records,
            Err(LookupFailed) => {
                return Err(OutboundPolicyError::new(PolicyFailure::Resolution));
            }
        };
        let addresses = admit_records(&records, port)?;
        self.pinned.insert(key, addresses.clone());
        Ok(addresses)
    }
}
