mod resources;
mod view;

use crate::{
    http_error::ApiError,
    models::{
        config::Config,
        lifecycle::Manager,
        model::{Backend, ModelId},
    },
};
use axum::{
    Json, Router,
    extract::{Path, Request, State},
    http::{HeaderMap, StatusCode, Uri, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Serialize;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::time::{Instant, timeout_at};

#[derive(Clone)]
struct App {
    config: Arc<Config>,
    manager: Manager,
}

pub(crate) fn router(config: Arc<Config>, manager: Manager) -> Router {
    let actions = Router::new()
        .route("/admin/models/{model}/load", post(load))
        .route("/admin/unload", post(unload))
        .route("/admin/ui/models/{model}/load", post(view::load))
        .route("/admin/ui/unload", post(view::unload))
        .route_layer(middleware::from_fn(same_origin));
    Router::new()
        .route("/", get(view::page))
        .route("/admin/ui/dashboard", get(view::fragment))
        .route(
            "/admin/htmx.js",
            get(|| async {
                asset(
                    "text/javascript; charset=utf-8",
                    include_str!("vendor/htmx-2.0.11.min.js"),
                )
            }),
        )
        .route(
            "/admin/app.js",
            get(|| async { asset("text/javascript; charset=utf-8", include_str!("app.js")) }),
        )
        .route(
            "/admin/style.css",
            get(|| async { asset("text/css; charset=utf-8", include_str!("style.css")) }),
        )
        .route(
            "/admin/theme.js",
            get(|| async { asset("text/javascript; charset=utf-8", include_str!("theme.js")) }),
        )
        .route("/admin/overview", get(overview))
        .route("/admin/status", get(status))
        .merge(actions)
        .layer(middleware::from_fn(no_cache))
        .with_state(App { config, manager })
}

fn asset(content_type: &'static str, content: impl Into<String>) -> Response {
    ([(header::CONTENT_TYPE, content_type),
      (header::CONTENT_SECURITY_POLICY, "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; base-uri 'none'; frame-ancestors 'none'; form-action 'none'"),
      (header::X_CONTENT_TYPE_OPTIONS, "nosniff")], content.into()).into_response()
}
async fn no_cache(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
}
async fn same_origin(request: Request, next: Next) -> Result<Response, ApiError> {
    let headers = request.headers();
    // A browser on another site can POST to loopback even without CORS permission.
    // Only the management page's own origin may initiate browser mutations.
    if headers
        .get("sec-fetch-site")
        .is_some_and(|v| v != "same-origin" && v != "none")
        || !origin_matches_host(headers)
    {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "cross_origin",
            "管理操作は同じ接続先の画面から実行してください。",
        ));
    }
    Ok(next.run(request).await)
}
fn origin_matches_host(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return true;
    };
    origin
        .to_str()
        .ok()
        .and_then(|s| s.parse::<Uri>().ok())
        .is_some_and(|uri| {
            uri.scheme_str() == Some("http")
                && uri.authority().map(|a| a.as_str())
                    == headers.get(header::HOST).and_then(|h| h.to_str().ok())
        })
}

#[derive(Serialize)]
struct ModelSummary<'a> {
    id: &'a ModelId,
    backend: &'static str,
}
async fn overview(State(app): State<App>) -> Json<Value> {
    let models: Vec<_> = app
        .config
        .models
        .values()
        .map(|model| ModelSummary {
            id: model.id(),
            backend: match model.backend {
                Backend::LlamaCpp { .. } => "llama_cpp",
                Backend::Strata { .. } => "strata",
            },
        })
        .collect();
    let resources = tokio::task::spawn_blocking(resources::collect).await.ok();
    Json(json!({
        "resources": resources,
        "status": app.manager.status(),
        "models": models,
        "queue": { "waiting": app.manager.waiting_requests(), "capacity": app.config.queue_capacity },
        "idle_timeout_secs": app.config.idle_timeout.as_secs(),
        "request_timeout_secs": app.config.request_timeout.as_secs(),
        "version": env!("CARGO_PKG_VERSION"),
    }))
}
async fn status(State(app): State<App>) -> Json<Value> {
    Json(json!({"model": app.manager.status()}))
}
async fn load(State(app): State<App>, Path(name): Path<String>) -> Result<Json<Value>, ApiError> {
    load_model(&app, &name).await?;
    Ok(Json(json!({"model": name, "status": "loaded"})))
}
async fn load_model(app: &App, name: &str) -> Result<(), ApiError> {
    let id = ModelId::parse(name)
        .map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, "invalid_model", e.to_string()))?;
    let model = app.config.models.get(&id).cloned().ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "model_not_found",
            format!("unknown model: {name}"),
        )
    })?;
    let lease = timeout_at(
        Instant::now() + app.config.request_timeout,
        app.manager.acquire(model),
    )
    .await
    .map_err(|_| ApiError::timeout())??;
    drop(lease);
    Ok(())
}
async fn unload(State(app): State<App>) -> Result<Json<Value>, ApiError> {
    unload_model(&app).await?;
    Ok(Json(json!({"status": "unloaded"})))
}
async fn unload_model(app: &App) -> Result<(), ApiError> {
    timeout_at(
        Instant::now() + app.config.request_timeout,
        app.manager.unload(),
    )
    .await
    .map_err(|_| ApiError::timeout())??;
    Ok(())
}
