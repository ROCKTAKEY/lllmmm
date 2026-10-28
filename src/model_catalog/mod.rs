use crate::models::{
    config::Config,
    model::{Model, ModelId},
};
use axum::{Json, Router, extract::State, routing::get};
use serde::Serialize;
use std::{num::NonZeroUsize, sync::Arc};

pub(crate) fn router(config: Arc<Config>) -> Router {
    Router::new()
        .route("/codex/models", get(catalog))
        .with_state(config)
}

async fn catalog(State(config): State<Arc<Config>>) -> Json<Catalog> {
    let mut models: Vec<_> = config
        .models
        .values()
        .map(|model| {
            let mut entry = Entry::from(model.as_ref());
            if config
                .default_model
                .as_ref()
                .is_some_and(|default| default.id() == model.id())
            {
                entry.priority = -1;
            }
            entry
        })
        .collect();
    models.sort_by_key(|entry| entry.priority);
    Json(Catalog { models })
}

#[derive(Serialize)]
struct Catalog {
    models: Vec<Entry>,
}

// Codex consumes ModelInfo rather than the OpenAI /v1/models object shape.
// Required fields follow https://github.com/openai/codex/blob/rust-v0.134.0/codex-rs/protocol/src/openai_models.rs
#[derive(Serialize)]
struct Entry {
    slug: ModelId,
    display_name: ModelId,
    description: &'static str,
    supported_reasoning_levels: [&'static str; 0],
    shell_type: &'static str,
    visibility: &'static str,
    supported_in_api: bool,
    priority: i32,
    base_instructions: &'static str,
    supports_reasoning_summaries: bool,
    supports_reasoning_summary_parameter: bool,
    support_verbosity: bool,
    supports_parallel_tool_calls: bool,
    truncation_policy: TruncationPolicy,
    experimental_supported_tools: [&'static str; 0],
    input_modalities: [&'static str; 1],
    context_window: NonZeroUsize,
}

#[derive(Serialize)]
#[serde(tag = "mode", content = "limit", rename_all = "snake_case")]
enum TruncationPolicy {
    Bytes(usize),
}

impl From<&Model> for Entry {
    fn from(model: &Model) -> Self {
        Self {
            slug: model.id().clone(),
            display_name: model.id().clone(),
            description: "Local model served by lllmmm",
            supported_reasoning_levels: [],
            shell_type: "shell_command",
            visibility: "list",
            supported_in_api: true,
            priority: 0,
            base_instructions: "You are a coding assistant. Use the available tools to inspect the project, make requested changes, and verify your work.",
            supports_reasoning_summaries: false,
            supports_reasoning_summary_parameter: false,
            support_verbosity: false,
            supports_parallel_tool_calls: false,
            truncation_policy: TruncationPolicy::Bytes(10000),
            experimental_supported_tools: [],
            input_modalities: ["text"],
            context_window: model.context_size(),
        }
    }
}
