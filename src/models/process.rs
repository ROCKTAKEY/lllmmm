use super::{
    config::Config,
    lifecycle::{Runtime, Session},
    model::{Backend, Model},
};
use anyhow::{Context, bail, ensure};
use async_trait::async_trait;
use nix::{
    errno::Errno,
    sys::signal::{Signal, killpg},
    unistd::Pid,
};
use std::{
    fs::OpenOptions, marker::PhantomData, path::PathBuf, process::Stdio, sync::Arc, time::Duration,
};
use tokio::{
    process::{Child, Command},
    time::{Instant, sleep, timeout},
};

pub struct ProcessRuntime {
    _ownership: std::fs::File,
    state_dir: PathBuf,
    startup_timeout: Duration,
    shutdown_timeout: Duration,
    health: reqwest::Client,
}
impl ProcessRuntime {
    pub fn new(config: &Config) -> anyhow::Result<Self> {
        std::fs::create_dir_all(&config.state_dir)?;
        let ownership = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(config.state_dir.join("owner.lock"))?;
        ownership
            .try_lock()
            .context("state directory already has an owner")?;
        Ok(Self {
            _ownership: ownership,
            state_dir: config.state_dir.clone(),
            startup_timeout: config.startup_timeout,
            shutdown_timeout: config.shutdown_timeout,
            health: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(2))
                .build()?,
        })
    }
}
struct Starting;
struct Ready;
struct Process<State> {
    group: ProcessGroup,
    endpoint: String,
    session_cache: Option<super::session_cache::SessionCache>,
    client: reqwest::Client,
    _state: PhantomData<State>,
}
struct ProcessGroup {
    child: Child,
    pid: Pid,
    grace: Duration,
    owned: bool,
}
impl ProcessGroup {
    fn signal(&self, signal: Signal) -> anyhow::Result<()> {
        match killpg(self.pid, signal) {
            Ok(()) | Err(Errno::ESRCH) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
    async fn stop(&mut self) -> anyhow::Result<()> {
        if !self.owned {
            return Ok(());
        }
        self.signal(Signal::SIGTERM)?;
        let graceful = async {
            self.child.wait().await?;
            while group_has_live_members(self.pid)? {
                sleep(Duration::from_millis(20)).await;
            }
            anyhow::Ok(())
        };
        match timeout(self.grace, graceful).await {
            Ok(result) => result?,
            Err(_) => {
                self.signal(Signal::SIGKILL)?;
                self.child.wait().await?;
            }
        }
        // Strata owns an engine child; waiting only for the HTTP server can leave VRAM occupied.
        self.signal(Signal::SIGKILL)?;
        timeout(Duration::from_secs(5), async {
            while group_has_live_members(self.pid)? {
                sleep(Duration::from_millis(20)).await;
            }
            anyhow::Ok(())
        })
        .await
        .context("backend process group did not exit")??;
        self.owned = false;
        Ok(())
    }
}
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        // Cancellation can drop startup before async cleanup runs; the process group
        // still needs a kill signal so the engine cannot outlive its owner.
        if self.owned {
            let _ = self.signal(Signal::SIGKILL);
        }
    }
}
fn group_has_live_members(group: Pid) -> anyhow::Result<bool> {
    for entry in std::fs::read_dir("/proc")? {
        let entry = entry?;
        if !entry
            .file_name()
            .to_string_lossy()
            .bytes()
            .all(|c| c.is_ascii_digit())
        {
            continue;
        }
        let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        let Some((_, fields)) = stat.rsplit_once(") ") else {
            continue;
        };
        let fields: Vec<_> = fields.split_whitespace().take(3).collect();
        if fields.len() == 3
            && fields[0] != "Z"
            && fields[0] != "X"
            && fields[2].parse::<i32>() == Ok(group.as_raw())
        {
            return Ok(true);
        }
    }
    Ok(false)
}
impl Process<Starting> {
    async fn ready(
        mut self,
        health: &reqwest::Client,
        model: &Model,
        limit: Duration,
    ) -> anyhow::Result<Process<Ready>> {
        let result = timeout(limit, async {
            loop {
                if let Some(status) = self.group.child.try_wait()? {
                    bail!("backend exited during startup: {status}");
                }
                if let Ok(response) = health.get(format!("{}/health", self.endpoint)).send().await
                    && response.status().is_success()
                    && let Ok(body) = response.json::<serde_json::Value>().await
                    && body.get("loaded").and_then(|v| v.as_bool()) != Some(false)
                    && let Ok(response) = health
                        .get(format!("{}/v1/models", self.endpoint))
                        .send()
                        .await
                    && response.status().is_success()
                    && let Ok(body) = response.json::<serde_json::Value>().await
                    && body["data"].as_array().is_some_and(|a| {
                        a.iter()
                            .any(|m| m["id"].as_str() == Some(model.id.as_str()))
                    })
                {
                    ensure!(
                        self.group.child.try_wait()?.is_none(),
                        "backend exited before readiness"
                    );
                    return anyhow::Ok(());
                }
                sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .context("backend startup timed out")
        .and_then(|r| r);
        if let Err(error) = result {
            self.group
                .stop()
                .await
                .context("cleanup after failed startup")?;
            return Err(error);
        }
        if let Some(cache) = &self.session_cache
            && let Err(error) = cache.restore(&self.client, &self.endpoint).await
        {
            self.group
                .stop()
                .await
                .context("cleanup after failed session restore")?;
            return Err(error);
        }
        Ok(Process {
            group: self.group,
            endpoint: self.endpoint,
            session_cache: self.session_cache,
            client: self.client,
            _state: PhantomData,
        })
    }
}
#[async_trait]
impl Runtime for ProcessRuntime {
    async fn start(&self, model: Arc<Model>) -> anyhow::Result<Box<dyn Session>> {
        // Fail instead of attaching to an unrelated server already listening on this port.
        let reservation = std::net::TcpListener::bind(("127.0.0.1", model.port.get()))
            .context("backend port is already in use")?;
        let log_path = self.state_dir.join(format!("{}.log", model.id.as_str()));
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)?;
        let mut command = Command::new(&model.executable);
        command
            .envs(&model.env)
            .env_remove("STRATA_API_KEY")
            .env_remove("LLAMA_API_KEY")
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .kill_on_drop(true)
            .process_group(0);
        match &model.backend {
            Backend::LlamaCpp {
                model_file,
                context_size,
                extra_args,
            } => {
                command
                    .args(extra_args)
                    .arg("--model")
                    .arg(model_file)
                    .args([
                        "--alias",
                        model.id.as_str(),
                        "--host",
                        "127.0.0.1",
                        "--port",
                        &model.port.to_string(),
                        "--ctx-size",
                        &context_size.to_string(),
                        "--parallel",
                        "1",
                        "--jinja",
                    ]);
            }
            Backend::Strata {
                native,
                session_cache,
                ..
            } => {
                let mut native = native.clone();
                if let Some(cache) = session_cache {
                    cache.configure(&mut native);
                }
                native["log"] = log_path.to_string_lossy().into_owned().into();
                let config = self.state_dir.join(format!("{}.json", model.id.as_str()));
                std::fs::write(&config, serde_json::to_vec_pretty(&native)?)?;
                command.arg("--config").arg(config).args([
                    "--host",
                    "127.0.0.1",
                    "--port",
                    &model.port.to_string(),
                    "--idle-unload",
                    "0",
                ]);
            }
        }
        drop(reservation);
        let child = command
            .spawn()
            .with_context(|| format!("start {}", model.executable.display()))?;
        let pid = Pid::from_raw(child.id().context("missing child PID")? as i32);
        let starting = Process::<Starting> {
            group: ProcessGroup {
                child,
                pid,
                grace: self.shutdown_timeout,
                owned: true,
            },
            endpoint: format!("http://127.0.0.1:{}", model.port),
            session_cache: match &model.backend {
                Backend::Strata { session_cache, .. } => session_cache.clone(),
                _ => None,
            },
            client: self.health.clone(),
            _state: PhantomData,
        };
        let started = Instant::now();
        let ready = starting
            .ready(&self.health, &model, self.startup_timeout)
            .await
            .with_context(|| {
                format!("loading {}; see {}", model.id.as_str(), log_path.display())
            })?;
        tracing::info!(
            model = model.id.as_str(),
            elapsed_secs = started.elapsed().as_secs_f32(),
            "model loaded"
        );
        Ok(Box::new(ready))
    }
}
#[async_trait]
impl Session for Process<Ready> {
    fn endpoint(&self) -> &str {
        &self.endpoint
    }
    fn is_alive(&mut self) -> bool {
        matches!(self.group.child.try_wait(), Ok(None))
    }
    async fn stop(&mut self) -> anyhow::Result<()> {
        let saved = if self.group.owned && self.is_alive() {
            if let Some(cache) = &self.session_cache {
                cache.save(&self.client, &self.endpoint).await
            } else {
                Ok(())
            }
        } else {
            Ok(())
        };
        self.group.stop().await?;
        saved
    }
}
