use super::{
    App,
    resources::{Memory, Resources},
};
use crate::models::{lifecycle::Status, model::Backend};
use askama::Template;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};

#[derive(Template)]
#[template(path = "page.html")]
struct Page {
    dashboard: Dashboard,
    version: &'static str,
}

#[derive(Template)]
#[template(path = "dashboard.html")]
struct Dashboard {
    state: StateView,
    waiting: usize,
    capacity: usize,
    idle_seconds: u64,
    request_timeout_ms: u128,
    models: Vec<ModelRow>,
    resources: Vec<ResourceCard>,
}

struct StateView {
    label: &'static str,
    model: String,
    description: &'static str,
    error: String,
    can_load: bool,
    can_unload: bool,
    loaded: bool,
    unknown: bool,
}
impl From<Status> for StateView {
    fn from(status: Status) -> Self {
        let (label, description) = match &status {
            Status::Unloaded => ("未ロード", "使用するモデルを一覧からロードできます。"),
            Status::Loading { .. } => ("ロード中", "モデルを準備しています。"),
            Status::Ready { .. } => ("待機中", "モデルを使用できます。"),
            Status::Busy { .. } => (
                "使用中",
                "要求を処理しています。モデルの操作は応答の終了を待ちます。",
            ),
            Status::Unloading { .. } => ("アンロード中", "モデルを終了しています。"),
            Status::Failed { .. } => ("エラー", "エラー内容を確認して、操作をやり直してください。"),
            Status::Stopped => ("停止", "lllmmmを起動してから、状態を更新してください。"),
        };
        let model = match &status {
            Status::Loading { model }
            | Status::Ready { model }
            | Status::Busy { model }
            | Status::Unloading { model } => model.clone(),
            _ => String::new(),
        };
        let can_load = matches!(
            status,
            Status::Unloaded | Status::Ready { .. } | Status::Busy { .. } | Status::Failed { .. }
        );
        Self {
            label,
            model,
            description,
            error: match &status {
                Status::Failed { message } => message.clone(),
                _ => String::new(),
            },
            can_load,
            can_unload: can_load && !matches!(status, Status::Unloaded),
            loaded: matches!(status, Status::Ready { .. } | Status::Busy { .. }),
            unknown: matches!(status, Status::Failed { .. } | Status::Stopped),
        }
    }
}
struct ModelRow {
    id: String,
    backend: &'static str,
    label: &'static str,
    active: bool,
    can_load: bool,
}
struct ResourceCard {
    icon: &'static str,
    name: String,
    fields: Vec<(&'static str, String)>,
}
fn memory_text(memory: Option<&Memory>) -> String {
    match memory {
        Some(memory) => format!(
            "{:.1} / {:.1} GiB（{:.0}%）",
            memory.used_bytes() as f64 / 1024_f64.powi(3),
            memory.total_bytes() as f64 / 1024_f64.powi(3),
            memory.used_bytes() as f64 / memory.total_bytes() as f64 * 100.0
        ),
        None => "取得不可".into(),
    }
}
fn resource_cards(resources: Option<Resources>) -> Vec<ResourceCard> {
    let mut cards = vec![ResourceCard {
        name: "DRAM".into(),
        icon: "memory",
        fields: vec![(
            "使用量 / 合計",
            memory_text(resources.as_ref().and_then(|r| r.memory.as_ref())),
        )],
    }];
    match resources.and_then(|r| r.gpus) {
        None => cards.push(ResourceCard {
            name: "GPU".into(),
            icon: "gpu",
            fields: vec![("状態", "取得不可".into())],
        }),
        Some(gpus) if gpus.is_empty() => cards.push(ResourceCard {
            name: "GPU".into(),
            icon: "gpu",
            fields: vec![("状態", "GPUは検出されませんでした".into())],
        }),
        Some(gpus) => cards.extend(gpus.into_iter().map(|gpu| ResourceCard {
            name: format!("{}（{}）", gpu.name, gpu.id),
            icon: "gpu",
            fields: vec![
                    (
                        "GPU使用率",
                        gpu.utilization_percent
                            .map(|p| format!("{p}%"))
                            .unwrap_or_else(|| "取得不可".into()),
                    ),
                    ("VRAM 使用量 / 合計", memory_text(gpu.vram.as_ref())),
                ],
        })),
    }
    cards
}
async fn dashboard(app: &App) -> Dashboard {
    let resources = tokio::task::spawn_blocking(super::resources::collect)
        .await
        .ok();
    let state = StateView::from(app.manager.status());
    let models = app
        .config
        .models
        .values()
        .map(|model| {
            let active = model.id().as_str() == state.model;
            ModelRow {
                id: model.id().as_str().into(),
                backend: match model.backend {
                    Backend::LlamaCpp { .. } => "llama.cpp",
                    Backend::Strata { .. } => "Strata",
                },
                label: if state.unknown {
                    "未確認"
                } else if active {
                    state.label
                } else {
                    "未ロード"
                },
                active,
                can_load: state.can_load && !(active && state.loaded),
            }
        })
        .collect();
    Dashboard {
        state,
        models,
        resources: resource_cards(resources),
        waiting: app.manager.waiting_requests(),
        capacity: app.config.queue_capacity,
        idle_seconds: app.config.idle_timeout.as_secs(),
        request_timeout_ms: app.config.request_timeout.as_millis().saturating_add(5000),
    }
}
fn html(template: &impl Template) -> Response {
    match template.render() {
        Ok(body) => super::asset("text/html; charset=utf-8", body),
        Err(error) => {
            tracing::error!(%error, "management template rendering failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "画面を生成できませんでした。",
            )
                .into_response()
        }
    }
}
pub(super) async fn page(State(app): State<App>) -> Response {
    html(&Page {
        dashboard: dashboard(&app).await,
        version: env!("CARGO_PKG_VERSION"),
    })
}
pub(super) async fn fragment(State(app): State<App>) -> Response {
    html(&dashboard(&app).await)
}

#[derive(Template)]
#[template(path = "operation.html")]
struct Operation {
    kind: &'static str,
    message: String,
}
fn operation(result: Result<(), crate::http_error::ApiError>, success: String) -> Response {
    let (status, notice) = match result {
        Ok(()) => (
            StatusCode::OK,
            Operation {
                kind: "success",
                message: success,
            },
        ),
        Err(error) => (
            error.status(),
            Operation {
                kind: "error",
                message: format!(
                    "操作に失敗しました：{} 状態を更新して確認してください。",
                    error.message()
                ),
            },
        ),
    };
    let mut response = html(&notice);
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert("hx-trigger", "refresh".parse().unwrap());
    response
}
pub(super) async fn load(State(app): State<App>, Path(name): Path<String>) -> Response {
    let result = super::load_model(&app, &name).await;
    operation(result, format!("{name} のロードが完了しました。"))
}
pub(super) async fn unload(State(app): State<App>) -> Response {
    let result = super::unload_model(&app).await;
    operation(result, "アンロードが完了しました。".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lifecycle_state_controls_available_actions() {
        let loading = StateView::from(Status::Loading { model: "a".into() });
        assert!(!loading.can_load && !loading.can_unload);
        let busy = StateView::from(Status::Busy { model: "a".into() });
        assert!(busy.can_load && busy.can_unload && busy.loaded);
        let failed = StateView::from(Status::Failed {
            message: "<script>alert(1)</script>".into(),
        });
        assert!(failed.can_load && failed.unknown);
        let notice = Operation {
            kind: "error",
            message: failed.error,
        }
        .render()
        .unwrap();
        assert!(!notice.contains("<script>"));
        assert!(notice.contains("alert(1)"));
    }
}
