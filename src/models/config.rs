use super::model::{Backend, Model, ModelId};
use anyhow::{Context, ensure};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    net::SocketAddr,
    num::{NonZeroU16, NonZeroU32, NonZeroUsize},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

#[derive(Debug)]
pub struct Config {
    pub(crate) listen: SocketAddr,
    pub(crate) state_dir: PathBuf,
    pub(crate) idle_timeout: Duration,
    pub(crate) startup_timeout: Duration,
    pub(crate) request_timeout: Duration,
    pub(crate) shutdown_timeout: Duration,
    pub(crate) queue_capacity: usize,
    pub(crate) max_body_bytes: usize,
    pub(crate) models: BTreeMap<ModelId, Arc<Model>>,
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawConfig {
    listen: SocketAddr,
    state_dir: PathBuf,
    idle_timeout_secs: NonZeroU32,
    startup_timeout_secs: NonZeroU32,
    request_timeout_secs: NonZeroU32,
    shutdown_timeout_secs: NonZeroU32,
    queue_capacity: NonZeroUsize,
    max_body_bytes: NonZeroUsize,
    models: Vec<RawModel>,
}
impl Default for RawConfig {
    fn default() -> Self {
        Self {
            listen: "127.0.0.1:8080".parse().unwrap(),
            state_dir: ".local".into(),
            idle_timeout_secs: NonZeroU32::new(900).unwrap(),
            startup_timeout_secs: NonZeroU32::new(600).unwrap(),
            request_timeout_secs: NonZeroU32::new(1800).unwrap(),
            shutdown_timeout_secs: NonZeroU32::new(10).unwrap(),
            queue_capacity: NonZeroUsize::new(32).unwrap(),
            max_body_bytes: NonZeroUsize::new(32 * 1024 * 1024).unwrap(),
            models: vec![],
        }
    }
}
#[derive(Deserialize)]
struct RawModel {
    id: String,
    executable: PathBuf,
    #[serde(default)]
    env: BTreeMap<String, String>,
    port: NonZeroU16,
    #[serde(flatten)]
    backend: RawBackend,
}
#[derive(Deserialize)]
#[serde(tag = "backend", rename_all = "snake_case", deny_unknown_fields)]
enum RawBackend {
    LlamaCpp {
        model_file: PathBuf,
        context_size: NonZeroUsize,
        #[serde(default)]
        extra_args: Vec<String>,
    },
    Strata {
        config_file: PathBuf,
    },
}
fn resolve(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_owned()
    } else {
        base.join(path)
    }
}

fn resolve_strata_engine(base: &Path, executable: &Path) -> anyhow::Result<PathBuf> {
    if executable.components().count() > 1 {
        return Ok(resolve(base, executable));
    }
    let paths = std::env::var_os("PATH").context("PATH is unset; cannot locate Strata engine")?;
    let path = std::env::split_paths(&paths)
        .map(|directory| directory.join(executable))
        .find(|candidate| {
            candidate.metadata().is_ok_and(|metadata| {
                metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
            })
        })
        .with_context(|| {
            format!(
                "Strata executable not found on PATH: {}",
                executable.display()
            )
        })?;
    Ok(std::path::absolute(path)?)
}

fn parse_strata_context_size(args: &[String]) -> anyhow::Result<NonZeroUsize> {
    let mut context_size = None;
    for (index, arg) in args.iter().enumerate() {
        ensure!(
            !arg.starts_with("--max-context="),
            "Strata --max-context requires a separate value"
        );
        if arg == "--max-context" {
            ensure!(context_size.is_none(), "duplicate Strata --max-context");
            context_size = Some(
                args.get(index + 1)
                    .context("missing Strata --max-context value")?
                    .parse::<NonZeroUsize>()
                    .context("Strata --max-context must be a positive integer")?,
            );
        }
    }
    Ok(context_size.unwrap_or(NonZeroUsize::new(4096).unwrap()))
}

impl Config {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let path = std::fs::canonicalize(path).context("locate configuration")?;
        Self::parse(&std::fs::read_to_string(&path)?, path.parent().unwrap())
    }
    pub fn listen(&self) -> SocketAddr {
        self.listen
    }
    pub fn model_ids(&self) -> impl Iterator<Item = &ModelId> {
        self.models.keys()
    }
    pub fn parse(text: &str, base: &Path) -> anyhow::Result<Self> {
        let raw: RawConfig = toml::from_str(text).context("parse lllmmm TOML")?;
        ensure!(
            base.is_absolute(),
            "configuration base directory must be absolute"
        );
        ensure!(
            raw.listen.ip().is_loopback() && raw.listen.port() != 0,
            "listen must use a loopback address and a nonzero port"
        );
        ensure!(!raw.models.is_empty(), "configure at least one model");
        let mut models = BTreeMap::new();
        for model in raw.models {
            let id = ModelId::parse(&model.id)?;
            ensure!(
                model.port.get() != raw.listen.port(),
                "backend port conflicts with listener"
            );
            ensure!(
                !model.executable.as_os_str().is_empty(),
                "executable is empty"
            );
            ensure!(
                model
                    .env
                    .keys()
                    .all(|s| !s.is_empty() && !s.contains(['=', '\0'])),
                "invalid environment variable name"
            );
            let backend = match model.backend {
                RawBackend::LlamaCpp {
                    model_file,
                    context_size,
                    extra_args,
                } => {
                    let reserved: BTreeSet<&str> = [
                        "-m",
                        "--model",
                        "-a",
                        "--alias",
                        "--host",
                        "--port",
                        "--api-key",
                        "--api-key-file",
                        "--models-dir",
                        "--models-preset",
                        "--models-max",
                        "--models-autoload",
                        "--ssl-key-file",
                        "--ssl-cert-file",
                    ]
                    .into_iter()
                    .collect();
                    ensure!(
                        !extra_args
                            .iter()
                            .any(|a| reserved.contains(a.split('=').next().unwrap_or(a))),
                        "extra_args must not override routing, authentication or model ownership"
                    );
                    ensure!(
                        resolve(base, &model_file).is_file(),
                        "model file does not exist: {}",
                        model_file.display()
                    );
                    Backend::LlamaCpp {
                        model_file: resolve(base, &model_file),
                        context_size,
                        extra_args,
                    }
                }
                RawBackend::Strata { config_file } => {
                    let file = resolve(base, &config_file);
                    let mut native: serde_json::Value = serde_json::from_str(
                        &std::fs::read_to_string(&file)
                            .with_context(|| format!("read {}", file.display()))?,
                    )?;
                    let object = native
                        .as_object_mut()
                        .context("Strata config must be an object")?;
                    for key in ["exe", "tokenizer"] {
                        let value = object
                            .get(key)
                            .and_then(|v| v.as_str())
                            .filter(|s| !s.is_empty())
                            .with_context(|| format!("Strata {key} must be a path"))?;
                        let path = if key == "exe" {
                            resolve_strata_engine(file.parent().unwrap(), Path::new(value))?
                        } else {
                            resolve(file.parent().unwrap(), Path::new(value))
                        };
                        ensure!(
                            if key == "exe" {
                                path.is_file()
                            } else {
                                path.is_dir()
                            },
                            "Strata {key} does not exist: {}",
                            path.display()
                        );
                        object.insert(key.into(), path.to_string_lossy().into_owned().into());
                    }
                    let args: Vec<String> = serde_json::from_value(
                        object
                            .get("args")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .context("Strata args must be an array of strings")?;
                    let context_size = parse_strata_context_size(&args)?;
                    let cwd = object
                        .get("cwd")
                        .map(|v| v.as_str().context("Strata cwd must be a path"))
                        .transpose()?
                        .unwrap_or(".");
                    let cwd = resolve(file.parent().unwrap(), Path::new(cwd));
                    ensure!(cwd.is_dir(), "Strata cwd does not exist: {}", cwd.display());
                    object.insert("cwd".into(), cwd.to_string_lossy().into_owned().into());
                    object.insert("model_name".into(), id.as_str().into());
                    object.insert("port".into(), model.port.get().into());
                    object.insert("lazy_load".into(), false.into());
                    object.insert("idle_unload_s".into(), 0.into());
                    ensure!(
                        !object.contains_key("before_load"),
                        "Strata before_load must not manage other processes"
                    );
                    ensure!(
                        object.get("api_key").is_none_or(|v| v.as_str() == Some("")),
                        "Strata backend must not require an API key"
                    );
                    Backend::Strata {
                        native,
                        context_size,
                    }
                }
            };
            let executable = if model.executable.components().count() > 1 {
                resolve(base, &model.executable)
            } else {
                model.executable
            };
            let spec = Arc::new(Model {
                id: id.clone(),
                executable,
                env: model.env,
                port: model.port,
                backend,
            });
            ensure!(models.insert(id, spec).is_none(), "duplicate model id");
        }
        Ok(Self {
            listen: raw.listen,
            state_dir: resolve(base, &raw.state_dir),
            idle_timeout: Duration::from_secs(raw.idle_timeout_secs.get().into()),
            startup_timeout: Duration::from_secs(raw.startup_timeout_secs.get().into()),
            request_timeout: Duration::from_secs(raw.request_timeout_secs.get().into()),
            shutdown_timeout: Duration::from_secs(raw.shutdown_timeout_secs.get().into()),
            queue_capacity: raw.queue_capacity.get(),
            max_body_bytes: raw.max_body_bytes.get(),
            models,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_model_identity_and_rejects_ambiguous_ownership() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("weights.gguf"), "fixture").unwrap();
        let valid = r#"[[models]]
id = "test"
backend = "llama_cpp"
executable = "llama-server"
model_file = "weights.gguf"
context_size = 32768
port = 18081
"#;
        assert_eq!(
            Config::parse(valid, dir.path())
                .unwrap()
                .model_ids()
                .next()
                .unwrap()
                .as_str(),
            "test"
        );
        for source in [
            "models = []".to_string(),
            valid.replace("id = \"test\"", "id = \"../escape\""),
            valid.replace("18081", "0"),
            valid.replace("18081", "8080"),
            valid.replace("32768", "0"),
            format!("{valid}extra_args = [\"--host=0.0.0.0\"]"),
            format!("{valid}unknown_option = true"),
            format!("listen = \"0.0.0.0:8080\"\n{valid}"),
            format!("{valid}\n{valid}"),
        ] {
            assert!(
                Config::parse(&source, dir.path()).is_err(),
                "accepted {source}"
            );
        }
    }
    #[test]
    fn rejects_invalid_or_ambiguous_strata_context_before_startup() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("engine"), "fixture").unwrap();
        let config = r#"[[models]]
id = "strata"
backend = "strata"
executable = "strata-server"
port = 18096
config_file = "native.json"
"#;
        for args in [
            serde_json::json!(["--max-context"]),
            serde_json::json!(["--max-context", "0"]),
            serde_json::json!(["--max-context", "-1"]),
            serde_json::json!(["--max-context", "invalid"]),
            serde_json::json!(["--max-context", "18446744073709551616"]),
            serde_json::json!(["--max-context", "32768", "--max-context", "262144"]),
            serde_json::json!(["--max-context=262144"]),
        ] {
            std::fs::write(
                dir.path().join("native.json"),
                serde_json::json!({"exe":"./engine", "tokenizer":".", "args":args}).to_string(),
            )
            .unwrap();
            let error = Config::parse(config, dir.path()).unwrap_err();
            assert!(format!("{error:#}").contains("--max-context"), "{error:#}");
        }
    }

    #[test]
    fn native_strata_paths_are_checked_before_starting_any_process() {
        let dir = tempfile::tempdir().unwrap();
        let native = dir.path().join("native.json");
        let config = r#"[[models]]
id = "strata"
backend = "strata"
executable = "strata-server"
port = 18096
config_file = "native.json"
"#;
        std::fs::write(
            &native,
            r#"{"exe":"missing-engine","tokenizer":".","args":[]}"#,
        )
        .unwrap();
        assert!(Config::parse(config, dir.path()).is_err());
        std::fs::write(dir.path().join("engine"), "fixture").unwrap();
        std::fs::write(
            &native,
            r#"{"exe":"./engine","tokenizer":"missing-tokenizer","args":[]}"#,
        )
        .unwrap();
        assert!(Config::parse(config, dir.path()).is_err());
        std::fs::write(&native, r#"{"exe":"./engine","tokenizer":".","args":[]}"#).unwrap();
        let parsed = Config::parse(config, dir.path()).unwrap();
        assert_eq!(
            parsed.models.values().next().unwrap().context_size().get(),
            4096
        );
    }
}
