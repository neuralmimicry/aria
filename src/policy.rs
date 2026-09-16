//! Deterministic checks are independent of model availability and cannot be downgraded by AI.
use crate::models::{Category, Evaluation, Finding};
use anyhow::{Result, ensure};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub revision: i64,
    pub paused: bool,
    pub ai_enabled: bool,
    pub require_ai: bool,
    pub alert_threshold: f64,
    pub block_threshold: f64,
    pub max_body_bytes: usize,
    pub block_uninspectable: bool,
    pub blocked_sources: Vec<String>,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            revision: 1,
            paused: false,
            ai_enabled: true,
            require_ai: true,
            alert_threshold: 0.45,
            block_threshold: 0.8,
            max_body_bytes: 262144,
            block_uninspectable: true,
            blocked_sources: vec![],
        }
    }
}

impl Policy {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.revision > 0, "invalid policy revision");
        ensure!(
            (0.0..=1.0).contains(&self.alert_threshold)
                && (0.0..=1.0).contains(&self.block_threshold)
                && self.alert_threshold < self.block_threshold,
            "thresholds must satisfy 0 <= alert < block <= 1"
        );
        ensure!(
            (1024..=1048576).contains(&self.max_body_bytes),
            "body limit must be between 1 KiB and 1 MiB"
        );
        ensure!(
            self.ai_enabled || !self.require_ai,
            "required AI cannot be disabled"
        );
        ensure!(
            self.blocked_sources.len() <= 1000
                && self
                    .blocked_sources
                    .iter()
                    .all(|s| !s.is_empty() && s.len() <= 100),
            "invalid blocked sources"
        );
        Ok(())
    }
}

static RULES: LazyLock<Vec<(Category, f64, &'static str, Regex)>> = LazyLock::new(|| {
    [
        (Category::PromptInjection, 0.9, "instruction_override", r"(?i)(ignore|disregard|override)\s+(all\s+)?(previous|prior|system|safety)\s+(instructions|rules|prompts)"),
        (Category::CredentialExposure, 0.95, "private_key", r"-----BEGIN (RSA |EC |OPENSSH )?PRIVATE KEY-----"),
        (Category::CredentialExposure, 0.9, "credential_pattern", r"(?i)(sk-[a-z0-9_-]{24,}|gh[pousr]_[a-z0-9]{30,}|AKIA[A-Z0-9]{16})"),
        (Category::DestructiveAction, 0.95, "destructive_command", r"(?i)(rm\s+-[rf]{1,4}\s+/(\s|$)|mkfs\.[a-z0-9]+\s+/dev/|:\(\)\s*\{\s*:\|:\s*&\s*\})"),
        (Category::PhysicalHarm, 0.85, "harm_instruction", r"(?i)(instructions|steps|guide)\s+(for|to)\s+(poison|kill|manufacture explosives)"),
    ].into_iter().map(|(c,s,n,r)| (c,s,n,Regex::new(r).expect("built-in rule is valid"))).collect()
});

pub fn inspect(input: &Evaluation, policy: &Policy) -> Vec<Finding> {
    let mut findings: Vec<Finding> = RULES
        .iter()
        .filter(|(_, _, _, r)| r.is_match(&input.content))
        .map(|(category, score, code, _)| Finding {
            category: *category,
            score: *score,
            code: (*code).into(),
        })
        .collect();
    let mut add = |category, score, code: &str| {
        findings.push(Finding {
            category,
            score,
            code: code.into(),
        })
    };
    if policy.paused {
        add(Category::Paused, 1.0, "governance_paused");
    }
    if policy.blocked_sources.contains(&input.source) {
        add(Category::SourceBlocked, 1.0, "source_blocked");
    }
    if input.body_bytes > policy.max_body_bytes {
        add(Category::ResourceAbuse, 1.0, "body_limit_exceeded");
    }
    if !input.inspection_complete {
        add(
            Category::UninspectableContent,
            if policy.block_uninspectable { 1.0 } else { 0.5 },
            "content_not_fully_inspected",
        );
    }
    findings
}
