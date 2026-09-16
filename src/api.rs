use crate::{
    auth::{self, Role},
    engine::Engine,
    models::Evaluation,
    policy::Policy,
};
use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Path, Query, Request, State},
    http::{HeaderValue, Method, StatusCode},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::{Arc, atomic::Ordering};
use tokio::sync::Semaphore;
use uuid::Uuid;

#[derive(Clone)]
struct ApiState {
    engine: Engine,
    requests: Arc<Semaphore>,
}

pub fn router(engine: Engine) -> Router {
    let state = ApiState {
        requests: Arc::new(Semaphore::new(engine.config.max_in_flight + 16)),
        engine,
    };
    let api = Router::new()
        .route("/v1/evaluate", post(evaluate))
        .route("/v1/session", get(session))
        .route("/v1/summary", get(summary))
        .route("/v1/gateway", get(gateway))
        .route("/v1/events", get(events))
        .route("/v1/events/{id}/acknowledge", post(acknowledge))
        .route("/v1/policy", get(policy).put(update_policy))
        .route("/v1/policy/history", get(history))
        .route("/v1/audit", get(audit))
        .route("/v1/products", get(products))
        .route("/metrics", get(metrics))
        .route_layer(middleware::from_fn_with_state(state.clone(), authorise));
    Router::new()
        .merge(api)
        .route(
            "/",
            get(|| async { Html(include_str!("../assets/index.html")) }),
        )
        .route(
            "/dashboard.js",
            get(|| async {
                (
                    [("content-type", "text/javascript; charset=utf-8")],
                    include_str!("../assets/dashboard.js"),
                )
            }),
        )
        .route(
            "/dashboard.css",
            get(|| async {
                (
                    [("content-type", "text/css; charset=utf-8")],
                    include_str!("../assets/dashboard.css"),
                )
            }),
        )
        .route(
            "/healthz",
            get(|| async { Json(json!({"ok":true,"service":"aria"})) }),
        )
        .route("/readyz", get(ready))
        .layer(DefaultBodyLimit::max(8 * 1024 * 1024))
        .layer(middleware::from_fn(security_headers))
        .with_state(state)
}

async fn authorise(State(state): State<ApiState>, mut request: Request, next: Next) -> Response {
    let Some(credential) = auth::authenticate(request.headers(), &state.engine.config.credentials)
    else {
        return error(StatusCode::UNAUTHORIZED, "authentication_required");
    };
    let path = request.uri().path();
    let permitted = if path == "/v1/evaluate" {
        credential.role == Role::Evaluator
    } else if path == "/v1/session" {
        true
    } else if request.method() == Method::GET {
        matches!(credential.role, Role::Viewer | Role::Operator)
    } else {
        credential.role == Role::Operator
    };
    if !permitted {
        return error(StatusCode::FORBIDDEN, "insufficient_permission");
    }
    let Ok(_permit) = state.requests.try_acquire() else {
        return error(StatusCode::TOO_MANY_REQUESTS, "capacity_exhausted");
    };
    request.extensions_mut().insert(Identity {
        principal: credential.principal.clone(),
        role: credential.role,
    });
    match tokio::time::timeout(std::time::Duration::from_secs(150), next.run(request)).await {
        Ok(response) => response,
        Err(_) => error(StatusCode::GATEWAY_TIMEOUT, "request_timeout"),
    }
}

#[derive(Clone)]
struct Identity {
    principal: String,
    role: Role,
}

async fn security_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    for (name, value) in [
        (
            "content-security-policy",
            "default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
        ),
        ("x-content-type-options", "nosniff"),
        ("referrer-policy", "no-referrer"),
        ("cache-control", "no-store"),
    ] {
        response
            .headers_mut()
            .insert(name, HeaderValue::from_static(value));
    }
    response
}

async fn evaluate(State(state): State<ApiState>, Json(input): Json<Evaluation>) -> Response {
    match state.engine.evaluate(input).await {
        Ok(decision) => Json(decision).into_response(),
        Err(err) => match err.to_string().as_str() {
            "capacity_exhausted" => error(StatusCode::TOO_MANY_REQUESTS, "capacity_exhausted"),
            "idempotency_conflict" => error(StatusCode::CONFLICT, "idempotency_conflict"),
            "invalid evaluation metadata" | "content exceeds limit" => {
                error(StatusCode::BAD_REQUEST, "invalid_evaluation")
            }
            _ => error(StatusCode::SERVICE_UNAVAILABLE, "evaluation_unavailable"),
        },
    }
}

async fn session(axum::Extension(identity): axum::Extension<Identity>) -> Json<Value> {
    Json(
        json!({"principal":identity.principal,"role":identity.role,"version":env!("CARGO_PKG_VERSION")}),
    )
}

async fn gateway(State(s): State<ApiState>) -> Json<Value> {
    Json(s.engine.gateway_status().await)
}

async fn summary(State(s): State<ApiState>) -> Response {
    result(s.engine.store.summary().await)
}
async fn policy(State(s): State<ApiState>) -> Response {
    result(s.engine.store.policy().await)
}
async fn history(State(s): State<ApiState>) -> Response {
    result(s.engine.store.history().await)
}
async fn audit(State(s): State<ApiState>) -> Response {
    result(s.engine.store.audit().await)
}
async fn products(State(s): State<ApiState>) -> Response {
    Json(&s.engine.config.products).into_response()
}

async fn update_policy(
    State(s): State<ApiState>,
    axum::Extension(identity): axum::Extension<Identity>,
    Json(policy): Json<Policy>,
) -> Response {
    if let Err(err) = policy.validate() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":err.to_string()})),
        )
            .into_response();
    }
    match s
        .engine
        .store
        .update_policy(policy, &identity.principal)
        .await
    {
        Ok(true) => result(s.engine.store.policy().await),
        Ok(false) => error(StatusCode::CONFLICT, "policy_changed_reload_before_saving"),
        Err(_) => error(StatusCode::SERVICE_UNAVAILABLE, "policy_store_unavailable"),
    }
}

#[derive(Default, Deserialize)]
struct EventQuery {
    limit: Option<i64>,
    before: Option<i64>,
    source: Option<String>,
    incidents: Option<bool>,
}
async fn events(State(s): State<ApiState>, Query(q): Query<EventQuery>) -> Response {
    result(
        s.engine
            .store
            .events(
                q.limit.unwrap_or(50),
                q.before.unwrap_or(i64::MAX),
                q.source.as_deref(),
                q.incidents.unwrap_or(false),
            )
            .await,
    )
}
async fn acknowledge(
    State(s): State<ApiState>,
    axum::Extension(identity): axum::Extension<Identity>,
    Path(id): Path<Uuid>,
) -> Response {
    match s.engine.store.acknowledge(id, &identity.principal).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => error(
            StatusCode::CONFLICT,
            "incident_missing_or_already_acknowledged",
        ),
        Err(_) => error(StatusCode::SERVICE_UNAVAILABLE, "audit_store_unavailable"),
    }
}
async fn ready(State(s): State<ApiState>) -> Response {
    // Gail readiness is deliberately excluded to avoid a circular startup dependency.
    if s.engine.store.policy().await.is_ok() {
        Json(json!({"ready":true})).into_response()
    } else {
        error(StatusCode::SERVICE_UNAVAILABLE, "database_unavailable")
    }
}
async fn metrics(State(s): State<ApiState>) -> Response {
    let Ok(summary) = s.engine.store.summary().await else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "metrics_unavailable");
    };
    let mut text = "# TYPE aria_decisions_total counter\n".to_string();
    for action in ["allow", "alert", "block"] {
        text.push_str(&format!(
            "aria_decisions_total{{action=\"{action}\"}} {}\n",
            summary["decisions"][action]
        ));
    }
    for key in ["pending_alerts", "failed_alerts", "unacknowledged"] {
        text.push_str(&format!(
            "# TYPE aria_{key} gauge\naria_{key} {}\n",
            summary[key]
        ));
    }
    text.push_str(&format!("# TYPE aria_ai_failures_total counter\naria_ai_failures_total {}\n# TYPE aria_capacity_rejections_total counter\naria_capacity_rejections_total {}\n",s.engine.ai_failures.load(Ordering::Relaxed),s.engine.rejected.load(Ordering::Relaxed)));
    Response::builder()
        .header("content-type", "text/plain; version=0.0.4")
        .body(Body::from(text))
        .expect("static response headers")
}
fn result<T: serde::Serialize>(value: anyhow::Result<T>) -> Response {
    match value {
        Ok(value) => Json(value).into_response(),
        Err(_) => error(StatusCode::SERVICE_UNAVAILABLE, "store_unavailable"),
    }
}
fn error(status: StatusCode, code: &str) -> Response {
    (status, Json(json!({"error":code}))).into_response()
}
