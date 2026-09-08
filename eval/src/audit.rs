//! Deterministic audit trails with hash chains for tamper detection.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::types::Invocation;

/// An [`Invocation`] wrapped with a hash chain for tamper detection.
///
/// Each turn is hashed individually, then the concatenated hashes are hashed
/// again to produce a single `chain_hash`. Use [`verify`](Self::verify) to
/// check integrity after deserialization or storage.
///
/// **Note:** `serde_json::Value` map key order is insertion-dependent, so audit
/// trails verify same-instance integrity, not cross-process reproducibility.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditedInvocation {
    /// The original invocation trace.
    pub invocation: Invocation,
    /// Hex-encoded SHA-256 of each turn's canonical JSON.
    pub turn_hashes: Vec<String>,
    /// Hex-encoded SHA-256 of all `turn_hashes` concatenated.
    pub chain_hash: String,
}

impl AuditedInvocation {
    /// Wrap an [`Invocation`] with computed hash chain.
    #[must_use]
    pub fn from_invocation(invocation: Invocation) -> Self {
        let turn_hashes: Vec<String> = invocation
            .turns
            .iter()
            .map(|turn| {
                let json = serde_json::to_string(turn).expect("TurnRecord is serializable");
                hex_sha256(json.as_bytes())
            })
            .collect();

        let chain_hash = compute_chain_hash(&turn_hashes);

        Self {
            invocation,
            turn_hashes,
            chain_hash,
        }
    }

    /// Recompute all hashes and verify they match the stored values.
    #[must_use]
    pub fn verify(&self) -> bool {
        if self.turn_hashes.len() != self.invocation.turns.len() {
            return false;
        }

        for (turn, stored_hash) in self.invocation.turns.iter().zip(&self.turn_hashes) {
            let json = serde_json::to_string(turn).expect("TurnRecord is serializable");
            let computed = hex_sha256(json.as_bytes());
            if &computed != stored_hash {
                return false;
            }
        }

        let computed_chain = compute_chain_hash(&self.turn_hashes);
        computed_chain == self.chain_hash
    }
}

fn hex_sha256(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let hash = hasher.finalize();
    let mut out = String::with_capacity(hash.len() * 2);
    for byte in hash {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

fn compute_chain_hash(turn_hashes: &[String]) -> String {
    let concatenated: String = turn_hashes.concat();
    hex_sha256(concatenated.as_bytes())
}

#[cfg(test)]
#[path = "audit_tests.rs"]
mod tests;
