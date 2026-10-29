use anyhow::{Context, ensure};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::{path::PathBuf, time::Duration};

#[derive(Clone, Debug)]
pub(crate) struct SessionCache {
    directory: PathBuf,
    timeout: Duration,
}

#[derive(Clone, Copy)]
enum Operation {
    Save,
    Restore,
}
impl Operation {
    fn action(self) -> &'static str {
        match self {
            Self::Save => "save",
            Self::Restore => "restore",
        }
    }
    fn token_field(self) -> &'static str {
        match self {
            Self::Save => "n_saved",
            Self::Restore => "n_restored",
        }
    }
}

impl SessionCache {
    pub(crate) fn parse(
        directory: PathBuf,
        timeout: Duration,
        native: &Map<String, Value>,
        args: &[String],
    ) -> anyhow::Result<Self> {
        ensure!(
            !native.contains_key("slot_save_path"),
            "session_cache owns slot_save_path; remove it from the Strata JSON"
        );
        ensure!(
            native
                .get("parallel")
                .is_none_or(|value| value.as_u64() == Some(1))
                && !native.contains_key("split_serve")
                && !native.contains_key("batch_groups"),
            "session_cache requires a single Strata slot without split serving"
        );
        for (index, argument) in args.iter().enumerate() {
            let (flag, inline) = argument
                .split_once('=')
                .map_or((argument.as_str(), None), |(flag, value)| {
                    (flag, Some(value))
                });
            ensure!(
                !matches!(
                    flag,
                    "--batch"
                        | "--slots"
                        | "--parallel"
                        | "--batch-groups"
                        | "--layer-split"
                        | "--peer-device"
                        | "--split-serve"
                ),
                "session_cache does not support Strata argument {flag}"
            );
            if flag == "--prompt-cache" {
                let value = inline.or_else(|| args.get(index + 1).map(String::as_str));
                ensure!(
                    value
                        .and_then(|v| v.parse::<std::num::NonZeroUsize>().ok())
                        .is_some(),
                    "session_cache requires --prompt-cache greater than zero"
                );
            }
        }
        Ok(Self { directory, timeout })
    }

    pub(crate) fn configure(&self, native: &mut Value) {
        native["slot_save_path"] = self.directory.to_string_lossy().into_owned().into();
    }

    pub(crate) async fn restore(
        &self,
        client: &reqwest::Client,
        endpoint: &str,
    ) -> anyhow::Result<()> {
        if !self
            .directory
            .join("session.bin")
            .try_exists()
            .context("inspect saved Strata session")?
        {
            return Ok(());
        }
        self.transfer(client, endpoint, Operation::Restore).await
    }

    pub(crate) async fn save(
        &self,
        client: &reqwest::Client,
        endpoint: &str,
    ) -> anyhow::Result<()> {
        self.transfer(client, endpoint, Operation::Save).await
    }

    async fn transfer(
        &self,
        client: &reqwest::Client,
        endpoint: &str,
        operation: Operation,
    ) -> anyhow::Result<()> {
        let action = operation.action();
        // Strata serializes session operations with generation and publishes saves atomically.
        // https://github.com/Niko1221/Strata/blob/main/docs/DETAILS.md#session-files-disk
        let response = client
            .post(format!("{endpoint}/slots/0?action={action}"))
            .timeout(self.timeout)
            .json(&json!({"filename": "session.bin"}))
            .send()
            .await
            .with_context(|| format!("Strata session {action} request"))?;
        let status = response.status();
        let body: Value = response
            .json()
            .await
            .context("read Strata session response")?;
        ensure!(
            status.is_success(),
            "Strata session {action} failed ({status}): {body}"
        );
        #[derive(Deserialize)]
        struct Receipt {
            id_slot: u32,
            filename: String,
        }
        let tokens = body[operation.token_field()].as_u64();
        let receipt: Receipt =
            serde_json::from_value(body).context("invalid Strata session receipt")?;
        ensure!(
            receipt.id_slot == 0 && receipt.filename == "session.bin" && tokens.is_some(),
            "invalid Strata session {action} receipt"
        );
        tracing::info!(action, path = %self.directory.join("session.bin").display(), "Strata session transferred");
        Ok(())
    }
}
