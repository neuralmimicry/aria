//! Durable, signed webhook delivery. A lease prevents concurrent replicas claiming a job twice.
use crate::{config::Config, models::now, store::Store};
use anyhow::Result;
use futures::{StreamExt, stream};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use sqlx::Row;

pub async fn deliver_batch(store: &Store, config: &Config, client: &reqwest::Client) -> Result<()> {
    let (Some(url), Some(secret)) = (&config.webhook_url, &config.webhook_secret) else {
        return Ok(());
    };
    let jobs = sqlx::query("SELECT id,body,attempts FROM aria_alerts WHERE delivered=0 AND attempts<8 AND available_at<=$1 AND lease_until<$1 ORDER BY available_at LIMIT 16")
        .bind(now()).fetch_all(&store.pool).await?;
    let results = stream::iter(jobs).map(|job| async move {
        let id: String = job.try_get("id")?;
        let body: String = job.try_get("body")?;
        let attempts: i64 = job.try_get("attempts")?;
        let claimed = sqlx::query("UPDATE aria_alerts SET lease_until=$1, attempts=attempts+1 WHERE id=$2 AND delivered=0 AND lease_until<$3 AND attempts=$4")
            .bind(now()+30).bind(&id).bind(now()).bind(attempts).execute(&store.pool).await?;
        if claimed.rows_affected() == 0 { return Ok::<(),anyhow::Error>(()); }
        let timestamp = now().to_string();
        let mut signer = Hmac::<Sha256>::new_from_slice(secret.as_bytes())?;
        signer.update(timestamp.as_bytes()); signer.update(b"."); signer.update(body.as_bytes());
        let signature = hex::encode(signer.finalize().into_bytes());
        let delivered = client.post(url).header("content-type","application/json")
            .header("x-aria-event-id",&id).header("x-aria-timestamp",timestamp)
            .header("x-aria-signature",format!("sha256={signature}"))
            .body(body).send().await.is_ok_and(|response| response.status().is_success());
        // Exponential delay plus stable jitter; exhausted jobs stay visible for investigation.
        let jitter = id.as_bytes().iter().map(|b| i64::from(*b)).sum::<i64>() % 7;
        sqlx::query("UPDATE aria_alerts SET delivered=$1,available_at=$2,lease_until=0 WHERE id=$3")
            .bind(i64::from(delivered)).bind(now() + (1_i64 << (attempts+1).min(10)) + jitter).bind(id).execute(&store.pool).await?;
        Ok(())
    }).buffer_unordered(4).collect::<Vec<_>>().await;
    for result in results {
        result?;
    }
    Ok(())
}

pub async fn run(
    store: Store,
    config: std::sync::Arc<Config>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = shutdown.changed() => return Ok(()),
            _ = interval.tick() => if deliver_batch(&store, &config, &client).await.is_err() {
                tracing::warn!("alert delivery batch failed; pending jobs remain durable");
            },
        }
    }
}
