use crate::{config::Config, gail::Gail, models::*, policy, store::Store};
use anyhow::{Result, bail, ensure};
use sha2::{Digest, Sha256};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use tokio::sync::Semaphore;
use uuid::Uuid;

#[derive(Clone)]
pub struct Engine {
    pub store: Store,
    pub config: Arc<Config>,
    gail: Gail,
    slots: Arc<Semaphore>,
    pub rejected: Arc<AtomicU64>,
    pub ai_failures: Arc<AtomicU64>,
}

impl Engine {
    pub async fn gateway_status(&self) -> serde_json::Value {
        self.gail
            .governance_status()
            .await
            .unwrap_or_else(|_| serde_json::json!({"available":false}))
    }
    pub fn new(config: Config, store: Store) -> Result<Self> {
        config.validate()?;
        Ok(Self {
            gail: Gail::new(&config)?,
            slots: Arc::new(Semaphore::new(config.max_in_flight)),
            config: Arc::new(config),
            store,
            rejected: Arc::new(AtomicU64::new(0)),
            ai_failures: Arc::new(AtomicU64::new(0)),
        })
    }

    pub async fn evaluate(&self, input: Evaluation) -> Result<Decision> {
        let started = std::time::Instant::now();
        ensure!(
            !input.source.is_empty()
                && input.source.len() <= 100
                && input.route.starts_with("/v1/")
                && input.route.len() <= 200,
            "invalid evaluation metadata"
        );
        ensure!(input.content.len() <= 1048576, "content exceeds limit");
        let Ok(_permit) = self.slots.try_acquire() else {
            self.rejected.fetch_add(1, Ordering::Relaxed);
            bail!("capacity_exhausted");
        };
        if let Some(existing) = self.store.existing(&input).await? {
            return Ok(existing);
        }
        let policy = self.store.policy().await?;
        let mut findings = policy::inspect(&input, &policy);
        let mut ai_status = "disabled".to_owned();
        let (mut provider, mut model) = (None, None);
        let deterministic_block = findings.iter().any(|f| f.score >= policy.block_threshold);
        if deterministic_block {
            ai_status = "skipped_blocked".into();
        } else if policy.ai_enabled {
            match self.gail.assess(&input).await {
                Ok(assessment) => {
                    ai_status = "assessed".into();
                    provider = Some(assessment.provider);
                    model = Some(assessment.model);
                    findings.extend(assessment.categories.into_iter().map(|category| Finding {
                        category,
                        score: assessment.score,
                        code: "ai_assessment".into(),
                    }));
                }
                Err(_) => {
                    // Never log upstream error bodies: they may echo inspected content.
                    self.ai_failures.fetch_add(1, Ordering::Relaxed);
                    ai_status = "unavailable".into();
                    findings.push(Finding {
                        category: Category::AiUnavailable,
                        score: if policy.require_ai {
                            1.0
                        } else {
                            policy.alert_threshold
                        },
                        code: "ai_unavailable".into(),
                    });
                }
            }
        }
        // A pause or source block activated during inference must still stop release.
        // Other policy changes take effect at the next evaluation boundary.
        let current = self.store.policy().await?;
        if current.revision != policy.revision {
            findings.extend(
                policy::inspect(&input, &current)
                    .into_iter()
                    .filter(|f| matches!(f.category, Category::Paused | Category::SourceBlocked)),
            );
        }
        let score = findings.iter().map(|f| f.score).fold(0.0, f64::max);
        let action = if score >= policy.block_threshold {
            Action::Block
        } else if score >= policy.alert_threshold {
            Action::Alert
        } else {
            Action::Allow
        };
        let decision = Decision {
            protocol_version: 1,
            id: Uuid::new_v4(),
            request_id: input.request_id,
            phase: input.phase,
            source: input.source.clone(),
            route: input.route.clone(),
            created_at: now(),
            policy_revision: policy.revision,
            control_revision: Some(current.revision),
            action,
            score,
            findings,
            ai_status,
            ai_provider: provider,
            ai_model: model,
            content_sha256: hex::encode(Sha256::digest(input.content.as_bytes())),
            body_bytes: input.body_bytes,
            evaluation_ms: started.elapsed().as_millis() as u64,
        };
        self.store
            .record(
                &input,
                &decision,
                action != Action::Allow && self.config.webhook_url.is_some(),
            )
            .await
    }
}
