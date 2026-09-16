use crate::auth::Credential;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{env, net::SocketAddr, time::Duration};

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Product {
    pub name: String,
    pub url: String,
}

#[derive(Clone)]
pub struct Config {
    pub bind: SocketAddr,
    pub database_url: String,
    pub credentials: Vec<Credential>,
    pub gail_url: String,
    pub gail_token: String,
    pub ai_timeout: Duration,
    pub max_in_flight: usize,
    pub webhook_url: Option<String>,
    pub webhook_secret: Option<String>,
    pub products: Vec<Product>,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let config = Self {
            bind: env::var("ARIA_BIND")
                .unwrap_or_else(|_| "127.0.0.1:8091".into())
                .parse()?,
            database_url: env::var("ARIA_DATABASE_URL")
                .unwrap_or_else(|_| "sqlite://aria.db?mode=rwc".into()),
            credentials: serde_json::from_str(
                &env::var("ARIA_TOKENS").context("ARIA_TOKENS is required")?,
            )?,
            gail_url: env::var("ARIA_GAIL_URL").unwrap_or_else(|_| "http://127.0.0.1:8080".into()),
            gail_token: env::var("ARIA_GAIL_TOKEN").context("ARIA_GAIL_TOKEN is required")?,
            ai_timeout: Duration::from_millis(number("ARIA_AI_TIMEOUT_MS", 15000)?),
            max_in_flight: number("ARIA_MAX_IN_FLIGHT", 32)? as usize,
            webhook_url: env::var("ARIA_WEBHOOK_URL").ok().filter(|s| !s.is_empty()),
            webhook_secret: env::var("ARIA_WEBHOOK_SECRET")
                .ok()
                .filter(|s| !s.is_empty()),
            products: serde_json::from_str(
                &env::var("ARIA_PRODUCTS")
                    .unwrap_or_else(|_| include_str!("../config/products.json").into()),
            )?,
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.credentials.is_empty(),
            "at least one API credential is required"
        );
        ensure!(
            self.gail_token.len() >= 32,
            "ARIA_GAIL_TOKEN must contain at least 32 characters"
        );
        for (i, credential) in self.credentials.iter().enumerate() {
            ensure!(
                credential.token.len() >= 32,
                "API tokens must contain at least 32 characters"
            );
            ensure!(
                !credential.principal.is_empty() && credential.principal.len() <= 100,
                "invalid principal"
            );
            ensure!(
                credential.token != self.gail_token,
                "Gail inference and Aria API credentials must differ"
            );
            ensure!(
                !self.credentials[..i]
                    .iter()
                    .any(|c| c.token == credential.token),
                "duplicate API token"
            );
        }
        ensure!(
            (1..=1024).contains(&self.max_in_flight),
            "invalid concurrency limit"
        );
        ensure!(
            (100..=120000).contains(&self.ai_timeout.as_millis()),
            "invalid AI timeout"
        );
        ensure!(
            self.database_url.starts_with("sqlite:") || self.database_url.starts_with("postgres"),
            "use SQLite or PostgreSQL"
        );
        validate_url(&self.gail_url)?;
        if let Some(url) = &self.webhook_url {
            validate_url(url)?;
            ensure!(
                self.webhook_secret.as_ref().is_some_and(|v| v.len() >= 32),
                "webhook signing secret must contain at least 32 characters"
            );
        }
        ensure!(self.products.len() <= 32, "too many product links");
        for product in &self.products {
            validate_url(&product.url)?;
        }
        Ok(())
    }
}

fn number(name: &str, fallback: u64) -> Result<u64> {
    env::var(name).map_or(Ok(fallback), |v| {
        v.parse().with_context(|| format!("invalid {name}"))
    })
}

pub fn validate_url(value: &str) -> Result<()> {
    let url = reqwest::Url::parse(value)?;
    ensure!(
        matches!(url.scheme(), "http" | "https")
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "invalid service URL"
    );
    Ok(())
}
