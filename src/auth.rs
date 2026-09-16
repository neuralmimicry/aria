//! Separate machine, monitoring and operator capabilities; tokens are never logged.
use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Evaluator,
    Viewer,
    Operator,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Credential {
    pub principal: String,
    pub role: Role,
    pub token: String,
}

pub fn authenticate<'a>(
    headers: &HeaderMap,
    credentials: &'a [Credential],
) -> Option<&'a Credential> {
    let token = headers
        .get("authorization")?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")?;
    let candidate = Sha256::digest(token.as_bytes());
    credentials
        .iter()
        .find(|c| bool::from(candidate.ct_eq(&Sha256::digest(c.token.as_bytes()))))
}
