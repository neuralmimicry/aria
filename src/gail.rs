//! No model SDK or Gail crate dependency. Only Gail's restricted assessment API is used.
use crate::{
    config::Config,
    models::{Category, Evaluation},
};
use anyhow::{Result, ensure};
use futures::StreamExt;
use serde::{Deserialize, Serialize};

#[derive(Clone)]
pub struct Gail {
    client: reqwest::Client,
    endpoint: String,
    token: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Assessment {
    pub score: f64,
    pub categories: Vec<Category>,
    pub provider: String,
    pub model: String,
}

impl Gail {
    pub async fn governance_status(&self) -> Result<serde_json::Value> {
        let endpoint = self.endpoint.trim_end_matches("/assess").to_owned() + "/status";
        let response = self
            .client
            .get(endpoint)
            .bearer_auth(&self.token)
            .timeout(std::time::Duration::from_secs(2))
            .send()
            .await?
            .error_for_status()?;
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            ensure!(
                bytes.len() + chunk.len() <= 4096,
                "gateway status exceeded limit"
            );
            bytes.extend_from_slice(&chunk);
        }
        let mut value: serde_json::Value = serde_json::from_slice(&bytes)?;
        ensure!(
            matches!(
                value["mode"].as_str(),
                Some("disabled" | "monitor" | "enforce")
            ),
            "invalid gateway status"
        );
        value["available"] = true.into();
        Ok(value)
    }
    pub fn new(config: &Config) -> Result<Self> {
        Ok(Self {
            client: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(2))
                .timeout(config.ai_timeout)
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            endpoint: format!(
                "{}/v1/internal/aria/assess",
                config.gail_url.trim_end_matches('/')
            ),
            token: config.gail_token.clone(),
        })
    }

    pub async fn assess(&self, input: &Evaluation) -> Result<Assessment> {
        let response = self
            .client
            .post(&self.endpoint)
            .bearer_auth(&self.token)
            .json(&serde_json::json!({"content":input.content,"phase":input.phase}))
            .send()
            .await?
            .error_for_status()?;
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            ensure!(
                bytes.len() + chunk.len() <= 16384,
                "assessment response exceeded limit"
            );
            bytes.extend_from_slice(&chunk);
        }
        let assessment: Assessment = serde_json::from_slice(&bytes)?;
        ensure!(
            (0.0..=1.0).contains(&assessment.score) && assessment.categories.len() <= 8,
            "invalid assessment"
        );
        ensure!(
            assessment.categories.iter().all(|c| matches!(
                c,
                Category::PromptInjection
                    | Category::CredentialExposure
                    | Category::DestructiveAction
                    | Category::PhysicalHarm
                    | Category::ResourceAbuse
            )),
            "invalid AI category"
        );
        ensure!(
            assessment.score == 0.0 || !assessment.categories.is_empty(),
            "positive risk requires a category"
        );
        ensure!(
            assessment.provider.len() <= 100 && assessment.model.len() <= 200,
            "invalid model provenance"
        );
        Ok(assessment)
    }
}
