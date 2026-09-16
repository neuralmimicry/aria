//! Shared SQL persistence. PostgreSQL supports replicas; SQLite is for one local instance.
use crate::{
    models::{Decision, Evaluation, Incident, now},
    policy::Policy,
};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{AnyPool, Row, any::AnyPoolOptions};
use uuid::Uuid;

#[derive(Clone)]
pub struct Store {
    pub pool: AnyPool,
}

const SCHEMA: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS aria_policy (id BIGINT PRIMARY KEY, revision BIGINT NOT NULL, body TEXT NOT NULL)",
    "CREATE TABLE IF NOT EXISTS aria_policy_history (revision BIGINT PRIMARY KEY, body TEXT NOT NULL, actor TEXT NOT NULL, created_at BIGINT NOT NULL)",
    "CREATE TABLE IF NOT EXISTS aria_events (id TEXT PRIMARY KEY, request_id TEXT NOT NULL, phase TEXT NOT NULL, source TEXT NOT NULL, fingerprint TEXT NOT NULL, action TEXT NOT NULL, body TEXT NOT NULL, created_at BIGINT NOT NULL, acknowledged_by TEXT, acknowledged_at BIGINT, UNIQUE(source, request_id, phase))",
    "CREATE INDEX IF NOT EXISTS aria_events_time ON aria_events(created_at, id)",
    "CREATE INDEX IF NOT EXISTS aria_events_action ON aria_events(action, created_at)",
    "CREATE TABLE IF NOT EXISTS aria_alerts (id TEXT PRIMARY KEY, body TEXT NOT NULL, attempts BIGINT NOT NULL DEFAULT 0, available_at BIGINT NOT NULL, lease_until BIGINT NOT NULL DEFAULT 0, delivered BIGINT NOT NULL DEFAULT 0)",
    "CREATE INDEX IF NOT EXISTS aria_alerts_pending ON aria_alerts(delivered, available_at, lease_until)",
    "CREATE TABLE IF NOT EXISTS aria_audit (id TEXT PRIMARY KEY, actor TEXT NOT NULL, operation TEXT NOT NULL, target TEXT NOT NULL, created_at BIGINT NOT NULL)",
];

impl Store {
    pub async fn connect(url: &str) -> Result<Self> {
        sqlx::any::install_default_drivers();
        // One connection serialises local SQLite writes without blocking Tokio workers.
        // PostgreSQL uses a bounded pool; compare-and-swap updates work across replicas.
        let pool = AnyPoolOptions::new()
            .max_connections(if url.starts_with("sqlite:") { 1 } else { 16 })
            .acquire_timeout(std::time::Duration::from_secs(3))
            .connect(url)
            .await
            .context("cannot connect to governance database")?;
        let mut tx = pool.begin().await?;
        // Serialise schema setup across PostgreSQL replicas during a rolling start.
        if url.starts_with("postgres") {
            sqlx::query("SELECT pg_advisory_xact_lock(715098231)")
                .execute(&mut *tx)
                .await?;
        }
        for statement in SCHEMA {
            sqlx::query(statement).execute(&mut *tx).await?;
        }
        let initial = serde_json::to_string(&Policy::default())?;
        sqlx::query("INSERT INTO aria_policy (id, revision, body) VALUES (1, 1, $1) ON CONFLICT (id) DO NOTHING")
            .bind(&initial).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO aria_policy_history (revision, body, actor, created_at) VALUES (1, $1, 'bootstrap', $2) ON CONFLICT (revision) DO NOTHING")
            .bind(&initial).bind(now()).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(Self { pool })
    }

    pub async fn policy(&self) -> Result<Policy> {
        let body: String = sqlx::query_scalar("SELECT body FROM aria_policy WHERE id = 1")
            .fetch_one(&self.pool)
            .await?;
        Ok(serde_json::from_str(&body)?)
    }

    /// Update the current policy and its immutable history in the same transaction.
    pub async fn update_policy(&self, mut policy: Policy, actor: &str) -> Result<bool> {
        policy.validate()?;
        let expected = policy.revision;
        policy.revision = expected
            .checked_add(1)
            .context("policy revision exhausted")?;
        let body = serde_json::to_string(&policy)?;
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query(
            "UPDATE aria_policy SET revision = $1, body = $2 WHERE id = 1 AND revision = $3",
        )
        .bind(policy.revision)
        .bind(&body)
        .bind(expected)
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() == 0 {
            return Ok(false);
        }
        sqlx::query("INSERT INTO aria_policy_history (revision, body, actor, created_at) VALUES ($1,$2,$3,$4)")
            .bind(policy.revision).bind(&body).bind(actor).bind(now()).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO aria_audit (id, actor, operation, target, created_at) VALUES ($1,$2,'policy_updated',$3,$4)")
            .bind(Uuid::new_v4().to_string()).bind(actor).bind(policy.revision.to_string()).bind(now()).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(true)
    }

    pub async fn existing(&self, input: &Evaluation) -> Result<Option<Decision>> {
        let row = sqlx::query("SELECT fingerprint, body FROM aria_events WHERE source=$1 AND request_id=$2 AND phase=$3")
            .bind(&input.source).bind(input.request_id.to_string()).bind(phase(input)).fetch_optional(&self.pool).await?;
        if let Some(row) = row {
            if row.try_get::<String, _>("fingerprint")? != fingerprint(input)? {
                bail!("idempotency_conflict");
            }
            return Ok(Some(serde_json::from_str(
                &row.try_get::<String, _>("body")?,
            )?));
        }
        Ok(None)
    }

    /// Persist the decision before releasing it, atomically enqueueing any alert.
    pub async fn record(
        &self,
        input: &Evaluation,
        decision: &Decision,
        alert: bool,
    ) -> Result<Decision> {
        let body = serde_json::to_string(decision)?;
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query("INSERT INTO aria_events (id,request_id,phase,source,fingerprint,action,body,created_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8) ON CONFLICT (source,request_id,phase) DO NOTHING")
            .bind(decision.id.to_string()).bind(input.request_id.to_string()).bind(phase(input)).bind(&input.source)
            .bind(fingerprint(input)?).bind(action(decision)).bind(&body).bind(decision.created_at).execute(&mut *tx).await?;
        if result.rows_affected() == 0 {
            tx.rollback().await?;
            return self
                .existing(input)
                .await?
                .context("concurrent decision disappeared");
        }
        if alert {
            sqlx::query("INSERT INTO aria_alerts (id,body,available_at) VALUES ($1,$2,$3)")
                .bind(decision.id.to_string())
                .bind(body)
                .bind(now())
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(decision.clone())
    }

    pub async fn events(
        &self,
        limit: i64,
        before: i64,
        source: Option<&str>,
        incidents: bool,
    ) -> Result<Vec<Incident>> {
        let rows = sqlx::query("SELECT body,acknowledged_by,acknowledged_at FROM aria_events WHERE created_at < $1 AND ($2 = '' OR source = $2) AND ($3 = 0 OR action <> 'allow') ORDER BY created_at DESC, id DESC LIMIT $4")
            .bind(before).bind(source.unwrap_or("")).bind(i64::from(incidents)).bind(limit.clamp(1,200)).fetch_all(&self.pool).await?;
        rows.into_iter()
            .map(|row| {
                Ok(Incident {
                    decision: serde_json::from_str(&row.try_get::<String, _>("body")?)?,
                    acknowledged_by: row.try_get("acknowledged_by")?,
                    acknowledged_at: row.try_get("acknowledged_at")?,
                })
            })
            .collect()
    }

    pub async fn acknowledge(&self, id: Uuid, actor: &str) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query("UPDATE aria_events SET acknowledged_by=$1, acknowledged_at=$2 WHERE id=$3 AND action <> 'allow' AND acknowledged_by IS NULL")
            .bind(actor).bind(now()).bind(id.to_string()).execute(&mut *tx).await?;
        if result.rows_affected() == 0 {
            return Ok(false);
        }
        sqlx::query("INSERT INTO aria_audit (id,actor,operation,target,created_at) VALUES ($1,$2,'incident_acknowledged',$3,$4)")
            .bind(Uuid::new_v4().to_string()).bind(actor).bind(id.to_string()).bind(now()).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(true)
    }

    pub async fn summary(&self) -> Result<Value> {
        let rows = sqlx::query("SELECT action, COUNT(*) AS total FROM aria_events GROUP BY action")
            .fetch_all(&self.pool)
            .await?;
        let mut counts = serde_json::json!({"allow":0,"alert":0,"block":0});
        for row in rows {
            counts[row.try_get::<String, _>("action")?] = row.try_get::<i64, _>("total")?.into();
        }
        let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM aria_alerts WHERE delivered=0")
            .fetch_one(&self.pool)
            .await?;
        let failed: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM aria_alerts WHERE delivered=0 AND attempts>=8",
        )
        .fetch_one(&self.pool)
        .await?;
        let unacknowledged: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM aria_events WHERE action <> 'allow' AND acknowledged_by IS NULL",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(
            serde_json::json!({"decisions":counts,"pending_alerts":pending,"failed_alerts":failed,"unacknowledged":unacknowledged}),
        )
    }

    pub async fn history(&self) -> Result<Vec<Value>> {
        let rows = sqlx::query("SELECT body,actor,created_at FROM aria_policy_history ORDER BY revision DESC LIMIT 100").fetch_all(&self.pool).await?;
        rows.into_iter().map(|r| Ok(serde_json::json!({"policy":serde_json::from_str::<Value>(&r.try_get::<String,_>("body")?)?,"actor":r.try_get::<String,_>("actor")?,"created_at":r.try_get::<i64,_>("created_at")?}))).collect()
    }

    pub async fn audit(&self) -> Result<Vec<Value>> {
        let rows = sqlx::query("SELECT actor,operation,target,created_at FROM aria_audit ORDER BY created_at DESC,id DESC LIMIT 100").fetch_all(&self.pool).await?;
        rows.into_iter().map(|r| Ok(serde_json::json!({"actor":r.try_get::<String,_>("actor")?,"operation":r.try_get::<String,_>("operation")?,"target":r.try_get::<String,_>("target")?,"created_at":r.try_get::<i64,_>("created_at")?}))).collect()
    }
}

fn phase(input: &Evaluation) -> &'static str {
    match input.phase {
        crate::models::Phase::Request => "request",
        crate::models::Phase::Response => "response",
    }
}
fn action(d: &Decision) -> &'static str {
    match d.action {
        crate::models::Action::Allow => "allow",
        crate::models::Action::Alert => "alert",
        crate::models::Action::Block => "block",
    }
}
fn fingerprint(input: &Evaluation) -> Result<String> {
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(input)?)))
}
