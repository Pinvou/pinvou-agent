//! Assistant attachment command compatibility facade.
//!
//! The implementation belongs to the assistant feature. Keeping this facade
//! lets command composition and focused tests share the same helpers without
//! introducing a feature -> app dependency.

pub(crate) use crate::features::assistant::attachments::*;
