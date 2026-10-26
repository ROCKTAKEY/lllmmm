use super::model::Model;
use async_trait::async_trait;
use std::{sync::Arc, time::Duration};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug)]
pub enum IdleTimeout {
    Disabled,
    After(std::num::NonZeroU32),
}
impl IdleTimeout {
    pub fn from_seconds(seconds: u32) -> Self {
        match std::num::NonZeroU32::new(seconds) {
            Some(seconds) => Self::After(seconds),
            None => Self::Disabled,
        }
    }
    pub fn as_secs(self) -> u64 {
        match self {
            Self::Disabled => 0,
            Self::After(seconds) => seconds.get().into(),
        }
    }
    fn deadline(self) -> Option<tokio::time::Instant> {
        match self {
            Self::Disabled => None,
            Self::After(seconds) => {
                Some(tokio::time::Instant::now() + Duration::from_secs(seconds.get().into()))
            }
        }
    }
}

async fn wait_for_idle(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

#[async_trait]
pub trait Runtime: Send + Sync + 'static {
    async fn start(&self, model: Arc<Model>) -> anyhow::Result<Box<dyn Session>>;
}
#[async_trait]
pub trait Session: Send {
    fn endpoint(&self) -> &str;
    fn is_alive(&mut self) -> bool;
    async fn stop(&mut self) -> anyhow::Result<()>;
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Status {
    Unloaded,
    Loading { model: String },
    Ready { model: String },
    Busy { model: String },
    Unloading { model: String },
    Failed { message: String },
    Stopped,
}
#[derive(Debug, thiserror::Error)]
pub enum LifecycleError {
    #[error("model request queue is full")]
    QueueFull,
    #[error("model manager is shutting down")]
    Stopped,
    #[error("backend lifecycle failed: {0}")]
    Backend(String),
}

// SSE generation continues after HTTP headers are returned. The response body
// must retain this lease so the manager cannot unload its backend mid-response.
pub struct Lease {
    pub endpoint: String,
    _release: oneshot::Sender<()>,
}
enum Command {
    Acquire {
        model: Arc<Model>,
        reply: oneshot::Sender<Result<Lease, LifecycleError>>,
    },
    Unload {
        reply: oneshot::Sender<Result<(), LifecycleError>>,
    },
}
#[derive(Clone)]
pub struct Manager {
    commands: mpsc::Sender<Command>,
    status: watch::Receiver<Status>,
    shutdown: CancellationToken,
}
impl Manager {
    pub fn spawn(
        runtime: Arc<dyn Runtime>,
        idle: IdleTimeout,
        capacity: usize,
    ) -> (Self, tokio::task::JoinHandle<()>) {
        let (commands, receiver) = mpsc::channel(capacity);
        let (status_tx, status) = watch::channel(Status::Unloaded);
        let shutdown = CancellationToken::new();
        let handle = tokio::spawn(run(runtime, receiver, status_tx, shutdown.clone(), idle));
        (
            Self {
                commands,
                status,
                shutdown,
            },
            handle,
        )
    }
    fn send(&self, command: Command) -> Result<(), LifecycleError> {
        if self.shutdown.is_cancelled() {
            return Err(LifecycleError::Stopped);
        }
        self.commands.try_send(command).map_err(|e| match e {
            mpsc::error::TrySendError::Full(_) => LifecycleError::QueueFull,
            mpsc::error::TrySendError::Closed(_) => LifecycleError::Stopped,
        })
    }
    pub async fn acquire(&self, model: Arc<Model>) -> Result<Lease, LifecycleError> {
        let (reply, received) = oneshot::channel();
        self.send(Command::Acquire { model, reply })?;
        received.await.map_err(|_| LifecycleError::Stopped)?
    }
    pub async fn unload(&self) -> Result<(), LifecycleError> {
        let (reply, received) = oneshot::channel();
        self.send(Command::Unload { reply })?;
        received.await.map_err(|_| LifecycleError::Stopped)?
    }
    pub fn waiting_requests(&self) -> usize {
        self.commands.max_capacity() - self.commands.capacity()
    }
    pub fn status(&self) -> Status {
        self.status.borrow().clone()
    }
    pub fn shutdown(&self) {
        self.shutdown.cancel();
    }
    pub fn cancellation(&self) -> CancellationToken {
        self.shutdown.clone()
    }
}

struct Resident {
    model: Arc<Model>,
    session: Box<dyn Session>,
}
async fn unload(
    resident: &mut Option<Resident>,
    status: &watch::Sender<Status>,
) -> Result<(), LifecycleError> {
    if let Some(active) = resident.as_mut() {
        status.send_replace(Status::Unloading {
            model: active.model.id.as_str().into(),
        });
        active
            .session
            .stop()
            .await
            .map_err(|e| LifecycleError::Backend(e.to_string()))?;
    }
    *resident = None;
    status.send_replace(Status::Unloaded);
    Ok(())
}
async fn prepare(
    runtime: &dyn Runtime,
    resident: &mut Option<Resident>,
    model: Arc<Model>,
    status: &watch::Sender<Status>,
) -> Result<String, LifecycleError> {
    if resident
        .as_mut()
        .is_some_and(|r| r.model.id != model.id || !r.session.is_alive())
    {
        unload(resident, status).await?;
    }
    if resident.is_none() {
        status.send_replace(Status::Loading {
            model: model.id.as_str().into(),
        });
        let session = runtime
            .start(model.clone())
            .await
            .map_err(|e| LifecycleError::Backend(format!("{e:#}")))?;
        *resident = Some(Resident { model, session });
    }
    Ok(resident.as_ref().unwrap().session.endpoint().to_owned())
}
async fn run(
    runtime: Arc<dyn Runtime>,
    mut commands: mpsc::Receiver<Command>,
    status: watch::Sender<Status>,
    shutdown: CancellationToken,
    idle: IdleTimeout,
) {
    let mut resident: Option<Resident> = None;
    let mut idle_deadline = idle.deadline();
    let mut health_poll = tokio::time::interval(Duration::from_secs(1));
    health_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        let command = tokio::select! {
            biased;
            _ = shutdown.cancelled() => break,
            command = commands.recv() => match command { Some(c) => c, None => break },
            _ = health_poll.tick(), if resident.is_some() => {
                if let Some(active) = resident.as_mut()
                    && !active.session.is_alive()
                {
                    let model = active.model.id.as_str().to_owned();
                    let message = match unload(&mut resident, &status).await {
                        Ok(()) => format!("backend {model} exited unexpectedly"),
                        Err(error) => format!("backend {model} exited; cleanup failed: {error}"),
                    };
                    status.send_replace(Status::Failed { message });
                }
                continue;
            }
            _ = wait_for_idle(idle_deadline), if resident.is_some() => {
                if let Err(error) = unload(&mut resident, &status).await {
                    status.send_replace(Status::Failed { message: error.to_string() });
                }
                idle_deadline = idle.deadline();
                continue;
            }
        };
        match command {
            Command::Acquire { model, reply } => {
                if reply.is_closed() {
                    continue;
                }
                let endpoint = tokio::select! {
                    biased;
                    _ = shutdown.cancelled() => break,
                    result = prepare(runtime.as_ref(), &mut resident, model.clone(), &status) => result,
                };
                match endpoint {
                    Ok(endpoint) => {
                        let (release, released) = oneshot::channel();
                        status.send_replace(Status::Busy {
                            model: model.id.as_str().into(),
                        });
                        let _ = reply.send(Ok(Lease {
                            endpoint,
                            _release: release,
                        }));
                        tokio::select! { _ = shutdown.cancelled() => break, _ = released => {} }
                        status.send_replace(Status::Ready {
                            model: model.id.as_str().into(),
                        });
                    }
                    Err(error) => {
                        status.send_replace(Status::Failed {
                            message: error.to_string(),
                        });
                        let _ = reply.send(Err(error));
                    }
                }
            }
            Command::Unload { reply } => {
                let result = unload(&mut resident, &status).await;
                if let Err(ref error) = result {
                    status.send_replace(Status::Failed {
                        message: error.to_string(),
                    });
                }
                let _ = reply.send(result);
            }
        }
        idle_deadline = idle.deadline();
    }
    commands.close();
    if let Err(error) = unload(&mut resident, &status).await {
        tracing::error!(%error, "shutdown cleanup failed");
    }
    status.send_replace(Status::Stopped);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::config::Config;
    use std::sync::Mutex;
    struct FakeRuntime(Arc<Mutex<Vec<String>>>);
    struct FakeSession {
        id: String,
        events: Arc<Mutex<Vec<String>>>,
    }
    #[async_trait]
    impl Runtime for FakeRuntime {
        async fn start(&self, model: Arc<Model>) -> anyhow::Result<Box<dyn Session>> {
            let id = model.id.as_str().to_owned();
            self.0.lock().unwrap().push(format!("start {id}"));
            Ok(Box::new(FakeSession {
                id,
                events: self.0.clone(),
            }))
        }
    }
    #[async_trait]
    impl Session for FakeSession {
        fn endpoint(&self) -> &str {
            "http://127.0.0.1:18081"
        }
        fn is_alive(&mut self) -> bool {
            true
        }
        async fn stop(&mut self) -> anyhow::Result<()> {
            self.events
                .lock()
                .unwrap()
                .push(format!("stop {}", self.id));
            Ok(())
        }
    }
    #[tokio::test]
    async fn switches_only_after_response_lease_is_released_and_reuses_loaded_model() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("weights.gguf"), "fixture").unwrap();
        let model = r#"[[models]]
id = "a"
backend = "llama_cpp"
executable = "unused"
model_file = "weights.gguf"
context_size = 32
port = 18081
"#;
        let config = Config::parse(
            &format!("{model}{}", model.replace("id = \"a\"", "id = \"b\"")),
            dir.path(),
        )
        .unwrap();
        let models: Vec<_> = config.models.values().cloned().collect();
        let events = Arc::new(Mutex::new(vec![]));
        let (manager, task) = Manager::spawn(
            Arc::new(FakeRuntime(events.clone())),
            IdleTimeout::from_seconds(60),
            2,
        );
        let a = manager.acquire(models[0].clone()).await.unwrap();
        let b = tokio::spawn({
            let manager = manager.clone();
            let model = models[1].clone();
            async move { manager.acquire(model).await }
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!b.is_finished());
        let (reply, cancelled) = oneshot::channel();
        manager
            .send(Command::Acquire {
                model: models[0].clone(),
                reply,
            })
            .unwrap();
        let (reply, _) = oneshot::channel();
        assert!(matches!(
            manager.send(Command::Unload { reply }),
            Err(LifecycleError::QueueFull)
        ));
        drop(cancelled);
        assert_eq!(*events.lock().unwrap(), ["start a"]);
        drop(a);
        drop(b.await.unwrap().unwrap());
        drop(manager.acquire(models[1].clone()).await.unwrap());
        manager.unload().await.unwrap();
        assert_eq!(
            *events.lock().unwrap(),
            ["start a", "stop a", "start b", "stop b"]
        );
        manager.shutdown();
        task.await.unwrap();
    }
}
