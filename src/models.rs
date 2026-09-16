use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Request,
    Response,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Evaluation {
    pub request_id: Uuid,
    pub phase: Phase,
    /// Gail derives this from its authenticated client, never an untrusted header.
    pub source: String,
    pub route: String,
    pub content: String,
    pub body_bytes: usize,
    pub inspection_complete: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    PromptInjection,
    CredentialExposure,
    DestructiveAction,
    PhysicalHarm,
    ResourceAbuse,
    UninspectableContent,
    AiUnavailable,
    Paused,
    SourceBlocked,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Finding {
    pub category: Category,
    pub score: f64,
    /// Stable explanation code; raw input and model prose are never retained.
    pub code: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Allow,
    Alert,
    Block,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Decision {
    pub protocol_version: u32,
    pub id: Uuid,
    pub request_id: Uuid,
    pub phase: Phase,
    pub source: String,
    pub route: String,
    pub created_at: i64,
    pub policy_revision: i64,
    /// Latest pause/source-block revision checked before release. Older records
    /// may lack this field; the rule/AI snapshot remains policy_revision.
    #[serde(default)]
    pub control_revision: Option<i64>,
    pub action: Action,
    pub score: f64,
    pub findings: Vec<Finding>,
    pub ai_status: String,
    pub ai_provider: Option<String>,
    pub ai_model: Option<String>,
    pub content_sha256: String,
    pub body_bytes: usize,
    pub evaluation_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Incident {
    pub decision: Decision,
    pub acknowledged_by: Option<String>,
    pub acknowledged_at: Option<i64>,
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
