//! Transcript content helpers shared by the store and engine surface.
//!
//! - [`transcript_revision`] is the stable optimistic-concurrency token for
//!   transcript content only: metadata and artifacts never participate, so
//!   renaming a Session or discovering an artifact cannot invalidate a browser
//!   transcript edit based on the same messages.
//! - [`looks_like_truncating_overwrite`] guards explicit store-maintenance
//!   flows against accidental truncation of an unrelated existing transcript.

use anyhow::{Context, Result};
use deepseek_tui::models::Message;
use sha2::{Digest, Sha256};

/// Stable optimistic-concurrency token for transcript content only.
///
/// Session metadata and artifacts intentionally do not participate: renaming a
/// Session or discovering an artifact must not invalidate a browser transcript
/// edit that was based on the same messages.
pub fn transcript_revision(messages: &[Message]) -> Result<String> {
    let encoded = serde_json::to_vec(messages).context("serialize transcript for revision")?;
    Ok(crate::platform::encoding::hex_lower(&Sha256::digest(
        encoded,
    )))
}

pub(crate) fn looks_like_truncating_overwrite(existing: &[Message], incoming: &[Message]) -> bool {
    if incoming.len() >= existing.len() || existing.len() <= 2 {
        return false;
    }
    let check = incoming.len().min(2);
    if check == 0 {
        return true;
    }
    for idx in 0..check {
        if existing[idx] != incoming[idx] {
            return true;
        }
    }
    false
}
