use anyhow::ensure;
use serde::Serialize;
use std::{
    collections::BTreeMap,
    num::{NonZeroU16, NonZeroUsize},
    path::PathBuf,
};

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ModelId(String);
impl ModelId {
    pub fn parse(value: &str) -> anyhow::Result<Self> {
        ensure!(
            !value.is_empty()
                && value.len() <= 128
                && value
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
                && value != "."
                && value != "..",
            "model id must be 1–128 ASCII letters, digits, '-', '_' or '.'"
        );
        Ok(Self(value.to_owned()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug)]
pub struct Model {
    pub(crate) id: ModelId,
    pub(crate) executable: PathBuf,
    pub(crate) env: BTreeMap<String, String>,
    pub(crate) port: NonZeroU16,
    pub(crate) backend: Backend,
}

#[derive(Debug)]
pub(crate) enum Backend {
    LlamaCpp {
        model_file: PathBuf,
        context_size: NonZeroUsize,
        extra_args: Vec<String>,
    },
    Strata {
        context_size: NonZeroUsize,
        native: serde_json::Value,
        session_cache: Option<super::session_cache::SessionCache>,
    },
}

impl Model {
    pub(crate) fn context_size(&self) -> NonZeroUsize {
        match &self.backend {
            Backend::LlamaCpp { context_size, .. } | Backend::Strata { context_size, .. } => {
                *context_size
            }
        }
    }

    pub fn id(&self) -> &ModelId {
        &self.id
    }
}
