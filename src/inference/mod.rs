use crate::http_error::ApiError;
use crate::models::{
    config::Config,
    lifecycle::{LifecycleError, Manager},
    model::{Model, ModelId},
};
use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::time::{Instant, timeout_at};

#[derive(Clone)]
struct App {
    config: Arc<Config>,
    manager: Manager,
    client: reqwest::Client,
}
pub fn router(config: Arc<Config>, manager: Manager) -> anyhow::Result<Router> {
    let management = crate::management::router(config.clone(), manager.clone());
    let app = App {
        config: config.clone(),
        manager,
        client: reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(std::time::Duration::from_secs(5))
            .build()?,
    };
    Ok(Router::new()
        .route("/health", get(|| async { Json(json!({"status":"ok"})) }))
        .route("/v1/models", get(models))
        .route("/v1/responses", post(responses))
        .route("/v1/chat/completions", post(chat))
        .fallback(|| async {
            ApiError::new(StatusCode::NOT_FOUND, "not_found", "unknown endpoint")
        })
        .layer(DefaultBodyLimit::max(config.max_body_bytes))
        .with_state(app)
        .merge(management))
}
fn json_payload(body: Result<Json<Value>, JsonRejection>) -> Result<Value, ApiError> {
    body.map(|Json(value)| value)
        .map_err(|error| ApiError::new(error.status(), "invalid_json", error.body_text()))
}
impl App {
    fn model(&self, name: &str) -> Result<Arc<Model>, ApiError> {
        let id = ModelId::parse(name)
            .map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, "invalid_model", e.to_string()))?;
        self.config.models.get(&id).cloned().ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "model_not_found",
                format!("unknown model: {name}"),
            )
        })
    }
}
async fn models(State(app): State<App>) -> Json<Value> {
    Json(
        json!({"object":"list","data": app.config.model_ids().map(|id| json!({"id":id,"object":"model","created":0,"owned_by":"lllmmm"})).collect::<Vec<_>>()}),
    )
}
async fn responses(
    State(app): State<App>,
    body: Result<Json<Value>, JsonRejection>,
) -> Result<Response, ApiError> {
    forward(app, "/v1/responses", json_payload(body)?).await
}
async fn chat(
    State(app): State<App>,
    body: Result<Json<Value>, JsonRejection>,
) -> Result<Response, ApiError> {
    forward(app, "/v1/chat/completions", json_payload(body)?).await
}
#[derive(Deserialize)]
struct RoutingRequest {
    model: String,
    #[serde(default)]
    stream: bool,
}
async fn forward(app: App, path: &str, body: Value) -> Result<Response, ApiError> {
    let routing: RoutingRequest = serde_json::from_value(body.clone())
        .map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, "invalid_request", e.to_string()))?;
    let model = app.model(&routing.model)?;
    let deadline = Instant::now() + app.config.request_timeout;
    let lease = timeout_at(deadline, app.manager.acquire(model))
        .await
        .map_err(|_| ApiError::timeout())??;
    let cancellation = app.manager.cancellation();
    let upstream = tokio::select! {
        _ = cancellation.cancelled() => return Err(LifecycleError::Stopped.into()),
        response = timeout_at(deadline, app.client.post(format!("{}{path}", lease.endpoint)).json(&body).send()) => response.map_err(|_| ApiError::timeout())?.map_err(|e| ApiError::new(StatusCode::BAD_GATEWAY, "upstream_error", e.to_string()))?,
    };
    let status = upstream.status();
    let mut headers = HeaderMap::new();
    for name in [
        "content-type",
        "cache-control",
        "retry-after",
        "x-request-id",
    ] {
        if let Some(value) = upstream.headers().get(name) {
            headers.insert(name, value.clone());
        }
    }
    if routing.stream {
        headers.insert("x-accel-buffering", "no".parse().unwrap());
    }
    let mut upstream = upstream.bytes_stream();
    let stream = async_stream::try_stream! {
        let _lease = lease;
        loop {
            let chunk = tokio::select! {
                _ = cancellation.cancelled() => Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "server shutting down")),
                chunk = timeout_at(deadline, upstream.next()) => match chunk {
                    Ok(Some(Ok(bytes))) => Ok(Some(bytes)),
                    Ok(Some(Err(error))) => Err(std::io::Error::other(error)),
                    Ok(None) => Ok(None),
                    Err(_) => Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "request deadline exceeded")),
                }
            }?;
            match chunk { Some(bytes) => yield bytes, None => break }
        }
    };
    let stream: std::pin::Pin<
        Box<dyn futures_util::Stream<Item = Result<axum::body::Bytes, std::io::Error>> + Send>,
    > = Box::pin(stream);
    Ok((status, headers, Body::from_stream(stream)).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::lifecycle::{Runtime, Session};
    use axum::{body::to_bytes, http::Request};
    use tower::ServiceExt;
    struct UnusedRuntime;
    #[async_trait::async_trait]
    impl Runtime for UnusedRuntime {
        async fn start(&self, _: Arc<Model>) -> anyhow::Result<Box<dyn Session>> {
            panic!("invalid requests must not load a model")
        }
    }
    #[tokio::test]
    async fn unknown_or_malformed_models_fail_before_loading() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("model.gguf"), "fixture").unwrap();
        let config = Arc::new(
            Config::parse(
                r#"[[models]]
id="known"
backend="llama_cpp"
executable="unused"
port=19090
context_size=32
model_file="model.gguf"
"#,
                dir.path(),
            )
            .unwrap(),
        );
        let (manager, task) = Manager::spawn(
            Arc::new(UnusedRuntime),
            std::time::Duration::from_secs(60),
            2,
        );
        let app = router(config, manager.clone()).unwrap();
        for (body, expected) in [
            (
                json!({"model":"unknown", "input":"hello"}),
                StatusCode::NOT_FOUND,
            ),
            (json!({"input":"hello"}), StatusCode::BAD_REQUEST),
            (
                json!({"model":"known", "stream":"yes"}),
                StatusCode::BAD_REQUEST,
            ),
        ] {
            let request = Request::post("/v1/responses")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap();
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), expected);
            let body: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap())
                    .unwrap();
            assert!(body["error"]["message"].is_string());
        }
        manager.shutdown();
        task.await.unwrap();
    }
}
