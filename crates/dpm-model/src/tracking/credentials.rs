//! Credential checks used by external tracker fields.

pub(super) use crate::credentials::check_text;
mod url;
pub(super) use url::check_url;

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
