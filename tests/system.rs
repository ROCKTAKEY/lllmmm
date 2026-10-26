use nix::{
    sys::signal::{Signal, kill},
    unistd::Pid,
};
use serde_json::{Value, json};
use std::{
    fs,
    net::TcpListener,
    process::{Child, Command},
    time::Duration,
};
use tokio::time::{sleep, timeout};

struct Server {
    child: Child,
    dir: tempfile::TempDir,
    url: String,
    client: reqwest::Client,
}
fn port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}
impl Server {
    async fn start(extra: &str) -> Self {
        Self::start_with_idle(extra, 1).await
    }
    async fn start_with_idle(extra: &str, idle: u64) -> Self {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("fixture.gguf"), "fixture").unwrap();
        fs::write(
            dir.path().join("strata.json"),
            json!({"exe":"./fixture.gguf", "tokenizer":".", "args":["--max-context", "262144", "--kv-resident", "32768"], "cwd":"."}).to_string(),
        )
        .unwrap();
        let listen = port();
        let backend = port();
        let python = std::env::split_paths(&std::env::var_os("PATH").unwrap())
            .map(|p| p.join("python3"))
            .find(|p| p.is_file())
            .unwrap();
        let executable = dir.path().join("backend.py");
        fs::write(
            &executable,
            include_str!("support/backend.py").replacen(
                "#!/usr/bin/env python3",
                &format!("#!{}", python.display()),
                1,
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let config = format!(
            r#"
listen = "127.0.0.1:{listen}"
idle_timeout_secs = {idle}
startup_timeout_secs = 2
request_timeout_secs = 5
shutdown_timeout_secs = 1
queue_capacity = 2
[[models]]
id = "llama"
backend = "llama_cpp"
executable = "{executable}"
port = {backend}
model_file = "fixture.gguf"
context_size = 32
[models.env]
EVENTS = "{events}"
{extra}
[[models]]
id = "strata"
backend = "strata"
executable = "{executable}"
port = {backend}
config_file = "strata.json"
[models.env]
EVENTS = "{events}"
"#,
            executable = executable.display(),
            events = dir.path().join("events").display()
        );
        fs::write(dir.path().join("lllmmm.toml"), config).unwrap();
        let log = fs::File::create(dir.path().join("server.log")).unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_lllmmm"))
            .arg("--config")
            .arg(dir.path().join("lllmmm.toml"))
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .unwrap();
        let server = Self {
            child,
            dir,
            url: format!("http://127.0.0.1:{listen}"),
            client: reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(10))
                .build()
                .unwrap(),
        };
        timeout(Duration::from_secs(5), async {
            loop {
                if server
                    .client
                    .get(format!("{}/health", server.url))
                    .send()
                    .await
                    .is_ok()
                {
                    break;
                }
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!(
                "server startup failed: {}",
                fs::read_to_string(server.dir.path().join("server.log")).unwrap()
            )
        });
        server
    }
    async fn post(&self, path: &str, body: Value) -> reqwest::Response {
        self.client
            .post(format!("{}{path}", self.url))
            .json(&body)
            .send()
            .await
            .unwrap()
    }
    fn events(&self) -> Vec<Value> {
        fs::read_to_string(self.dir.path().join("events"))
            .unwrap_or_default()
            .lines()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect()
    }
    fn assert_processes_stopped(&self) {
        for event in self.events() {
            for key in ["pid", "child"] {
                if let Ok(stat) = fs::read_to_string(format!("/proc/{}/stat", event[key])) {
                    assert!(
                        stat.rsplit_once(") ").unwrap().1.starts_with('Z'),
                        "{key} is still alive: {stat}"
                    );
                }
            }
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = kill(Pid::from_raw(self.child.id() as i32), Signal::SIGTERM);
        for _ in 0..100 {
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[tokio::test]
async fn codex_catalog_lists_configured_models_without_loading_them() {
    let server = Server::start("").await;
    let response = server
        .client
        .get(format!(
            "{}/codex/models?client_version=0.134.0",
            server.url
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-type"], "application/json");
    let catalog: Value = response.json().await.unwrap();
    let models = catalog["models"].as_array().unwrap();
    assert_eq!(models.len(), 2);
    for (model, id) in models.iter().zip(["llama", "strata"]) {
        assert_eq!(model["slug"], id);
        assert_eq!(model["display_name"], id);
        assert_eq!(model["visibility"], "list");
        assert_eq!(model["supported_in_api"], true);
        assert_eq!(model["shell_type"], "shell_command");
        assert_eq!(model["supported_reasoning_levels"], json!([]));
        assert_eq!(model["supports_reasoning_summaries"], false);
        assert_eq!(model["support_verbosity"], false);
        assert_eq!(model["supports_parallel_tool_calls"], false);
        assert_eq!(model["input_modalities"], json!(["text"]));
        assert_eq!(model["experimental_supported_tools"], json!([]));
        assert!(
            model["base_instructions"]
                .as_str()
                .is_some_and(|s| !s.is_empty())
        );
        assert!(model["priority"].is_i64());
        assert_eq!(
            model["truncation_policy"],
            json!({"mode":"bytes", "limit":10000})
        );
    }
    assert_eq!(models[0]["context_window"], 32);
    assert_eq!(models[1]["context_window"], 262144);
    assert!(server.events().is_empty());
}

#[tokio::test]
async fn native_protocols_switch_reuse_stream_and_unload_process_groups() {
    let server = Server::start("").await;
    let models: Value = server
        .client
        .get(format!("{}/v1/models", server.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(models["data"].as_array().unwrap().len(), 2);
    assert!(server.events().is_empty());
    let payload = json!({"model":"llama", "input":[{"type":"function_call_output","call_id":"call_1","output":"42"}], "tools":[{"type":"function","name":"shell","parameters":{"type":"object"}}], "store":false});
    let result = server.post("/v1/responses", payload.clone()).await;
    assert_eq!(result.status(), 200);
    assert_eq!(result.json::<Value>().await.unwrap()["request"], payload);
    let result = server
        .post(
            "/v1/chat/completions",
            json!({"model":"llama","messages":[]}),
        )
        .await;
    assert_eq!(
        result.json::<Value>().await.unwrap()["path"],
        "/v1/chat/completions"
    );
    assert_eq!(server.events().len(), 1);
    let mut stream = server
        .post(
            "/v1/responses",
            json!({"model":"llama","stream":true,"input":"hello"}),
        )
        .await;
    assert!(
        String::from_utf8(stream.chunk().await.unwrap().unwrap().to_vec())
            .unwrap()
            .contains("response.created")
    );
    let next = tokio::spawn({
        let client = server.client.clone();
        let url = server.url.clone();
        async move {
            client
                .post(format!("{url}/v1/responses"))
                .json(&json!({"model":"strata","input":"next"}))
                .send()
                .await
                .unwrap()
        }
    });
    sleep(Duration::from_millis(80)).await;
    assert_eq!(server.events().len(), 1);
    assert!(!next.is_finished());
    let remaining = stream.text().await.unwrap();
    assert!(remaining.contains("日本語") && remaining.contains("response.completed"));
    assert_eq!(
        next.await.unwrap().json::<Value>().await.unwrap()["model"],
        "strata"
    );
    assert_eq!(server.events().len(), 2);
    let error = server
        .post("/v1/responses", json!({"model":"strata","input":"error"}))
        .await;
    assert_eq!(error.status(), 400);
    assert_eq!(
        error.json::<Value>().await.unwrap()["error"]["code"],
        "context_length_exceeded"
    );
    assert_eq!(server.post("/admin/unload", json!({})).await.status(), 200);
    server.assert_processes_stopped();
}

#[tokio::test]
async fn zero_idle_timeout_keeps_model_loaded_and_allows_explicit_unload() {
    let server = Server::start_with_idle("", 0).await;
    assert_eq!(
        server
            .post("/admin/models/llama/load", json!({}))
            .await
            .status(),
        200
    );
    sleep(Duration::from_secs(2)).await;
    let snapshot: Value = server
        .client
        .get(format!("{}/admin/overview", server.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(snapshot["status"]["state"], "ready");
    assert_eq!(snapshot["idle_timeout_secs"], 0);
    let dashboard = server
        .client
        .get(format!("{}/admin/ui/dashboard", server.url))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(dashboard.contains("無効"));
    assert_eq!(
        server
            .post("/admin/models/strata/load", json!({}))
            .await
            .status(),
        200
    );
    assert_eq!(server.post("/admin/unload", json!({})).await.status(), 200);
    server.assert_processes_stopped();
}

#[tokio::test]
async fn idle_unload_and_cancelled_stream_allow_next_request() {
    let server = Server::start("").await;
    let mut response = server
        .post(
            "/v1/responses",
            json!({"model":"llama","stream":true,"delay":2}),
        )
        .await;
    response.chunk().await.unwrap().unwrap();
    drop(response);
    let next = server
        .post("/v1/responses", json!({"model":"strata","input":"hello"}))
        .await;
    assert_eq!(next.status(), 200);
    drop(next);
    timeout(Duration::from_secs(5), async {
        loop {
            let status: Value = server
                .client
                .get(format!("{}/admin/status", server.url))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            if status["model"]["state"] == "unloaded" {
                break;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    server.assert_processes_stopped();
}

#[tokio::test]
async fn startup_failure_and_timeout_release_resources_for_another_model() {
    for extra in ["FAIL_START = \"1\"", "READY_DELAY = \"30\""] {
        let server = Server::start(extra).await;
        let result = server
            .post("/v1/responses", json!({"model":"llama","input":"hello"}))
            .await;
        assert_eq!(result.status(), 503);
        server.assert_processes_stopped();
        let next = server
            .post("/v1/responses", json!({"model":"strata","input":"hello"}))
            .await;
        assert_eq!(next.status(), 200);
        assert_eq!(server.post("/admin/unload", json!({})).await.status(), 200);
        server.assert_processes_stopped();
    }
}

#[tokio::test]
async fn shutdown_during_inference_reaps_server_and_engine() {
    let mut server = Server::start("").await;
    let mut response = server
        .post(
            "/v1/responses",
            json!({"model":"strata","stream":true,"delay":30}),
        )
        .await;
    response.chunk().await.unwrap().unwrap();
    kill(Pid::from_raw(server.child.id() as i32), Signal::SIGTERM).unwrap();
    timeout(Duration::from_secs(5), async {
        while server.child.try_wait().unwrap().is_none() {
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    server.assert_processes_stopped();
}

#[tokio::test]
async fn occupied_backend_port_is_not_adopted_or_stopped() {
    let server = Server::start("").await;
    let config: toml::Value =
        toml::from_str(&fs::read_to_string(server.dir.path().join("lllmmm.toml")).unwrap())
            .unwrap();
    let occupied = TcpListener::bind((
        "127.0.0.1",
        config["models"][0]["port"].as_integer().unwrap() as u16,
    ))
    .unwrap();
    let response = server
        .post("/v1/responses", json!({"model":"llama","input":"hello"}))
        .await;
    assert_eq!(response.status(), 503);
    assert!(server.events().is_empty());
    assert!(occupied.local_addr().is_ok());
}

#[tokio::test]
async fn same_state_directory_cannot_have_two_process_owners() {
    let server = Server::start("").await;
    let config = fs::read_to_string(server.dir.path().join("lllmmm.toml")).unwrap();
    let second = config.replace(
        &server.url.replace("http://", ""),
        &format!("127.0.0.1:{}", port()),
    );
    fs::write(server.dir.path().join("second.toml"), second).unwrap();
    let log = fs::File::create(server.dir.path().join("second.log")).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_lllmmm"))
        .arg("--config")
        .arg(server.dir.path().join("second.toml"))
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .spawn()
        .unwrap();
    let exit = timeout(Duration::from_secs(2), async {
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    if exit.is_err() {
        let _ = kill(Pid::from_raw(child.id() as i32), Signal::SIGTERM);
        let _ = child.wait();
    }
    assert!(!exit.expect("second owner must be rejected").success());
    assert!(
        fs::read_to_string(server.dir.path().join("second.log"))
            .unwrap()
            .contains("state directory")
    );
}

#[tokio::test]
async fn stream_deadline_does_not_fabricate_success_or_pin_the_model() {
    let server = Server::start("").await;
    let mut response = server
        .post(
            "/v1/responses",
            json!({"model":"llama","stream":true,"delay":30}),
        )
        .await;
    let first = response.chunk().await.unwrap().unwrap();
    assert!(String::from_utf8_lossy(&first).contains("response.created"));
    assert!(response.text().await.is_err());
    let next = server
        .post("/v1/responses", json!({"model":"strata","input":"next"}))
        .await;
    assert_eq!(next.status(), 200);
}

#[tokio::test]
async fn management_screen_reports_models_and_controls_their_lifecycle() {
    let server = Server::start("").await;
    let page = server
        .client
        .get(format!("{}/", server.url))
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), 200);
    assert!(
        page.headers()["content-type"]
            .to_str()
            .unwrap()
            .contains("text/html")
    );
    let html = page.text().await.unwrap();
    assert!(html.contains("モデル管理"));
    assert!(html.contains("class=\"icon\""));
    assert!(html.contains("aria-hidden=\"true\" focusable=\"false\""));
    assert!(html.contains("今すぐ更新"));
    assert!(html.contains("llama"));
    assert!(html.contains("hx-post=\"/admin/ui/models/llama/load\""));
    let fragment = server
        .client
        .get(format!("{}/admin/ui/dashboard", server.url))
        .send()
        .await
        .unwrap();
    assert_eq!(fragment.status(), 200);
    assert!(fragment.text().await.unwrap().contains("DRAM"));
    for path in ["/admin/app.js", "/admin/style.css"] {
        assert_eq!(
            server
                .client
                .get(format!("{}{path}", server.url))
                .send()
                .await
                .unwrap()
                .status(),
            200
        );
    }
    let snapshot: Value = server
        .client
        .get(format!("{}/admin/overview", server.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(snapshot["status"]["state"], "unloaded");
    assert_eq!(snapshot["queue"]["waiting"], 0);
    assert_eq!(snapshot["queue"]["capacity"], 2);
    assert!(snapshot["resources"].is_object());
    assert!(snapshot["resources"].get("memory").is_some());
    assert!(snapshot["resources"].get("gpus").is_some());
    assert_eq!(snapshot["models"][0]["id"], "llama");
    assert_eq!(snapshot["models"][0]["backend"], "llama_cpp");
    assert_eq!(snapshot["models"][1]["backend"], "strata");
    assert!(!snapshot.to_string().contains("fixture.gguf"));
    assert!(server.events().is_empty());
    let blocked = server
        .client
        .post(format!("{}/admin/models/llama/load", server.url))
        .header("origin", "https://unrelated.example")
        .send()
        .await
        .unwrap();
    assert_eq!(blocked.status(), 403);
    assert!(server.events().is_empty());
    let loaded = server
        .client
        .post(format!("{}/admin/models/llama/load", server.url))
        .header("origin", &server.url)
        .send()
        .await
        .unwrap();
    assert_eq!(loaded.status(), 200);
    let snapshot: Value = server
        .client
        .get(format!("{}/admin/overview", server.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(snapshot["status"]["model"], "llama");
    assert_eq!(snapshot["status"]["state"], "ready");
    assert_eq!(server.post("/admin/unload", json!({})).await.status(), 200);
    let snapshot: Value = server
        .client
        .get(format!("{}/admin/overview", server.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(snapshot["status"]["state"], "unloaded");
}

#[tokio::test]
async fn monitoring_detects_a_backend_exit_while_idle() {
    let server = Server::start_with_idle("", 60).await;
    assert_eq!(
        server
            .post("/admin/models/llama/load", json!({}))
            .await
            .status(),
        200
    );
    let pid = server.events()[0]["pid"].as_i64().unwrap() as i32;
    kill(Pid::from_raw(pid), Signal::SIGKILL).unwrap();
    timeout(Duration::from_secs(5), async {
        loop {
            let snapshot: Value = server
                .client
                .get(format!("{}/admin/overview", server.url))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            if snapshot["status"]["state"] == "failed" {
                break;
            }
            sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .expect("monitor must report the exited backend before the idle timeout");
    server.assert_processes_stopped();
}

#[tokio::test]
async fn management_queue_waits_for_stream_and_remains_observable() {
    let server = Server::start("").await;
    let mut stream = server
        .post(
            "/v1/responses",
            json!({"model":"llama", "stream":true, "delay":1}),
        )
        .await;
    stream.chunk().await.unwrap();
    let client = server.client.clone();
    let url = format!("{}/admin/unload", server.url);
    let unload = tokio::spawn(async move { client.post(url).send().await.unwrap() });
    timeout(Duration::from_secs(1), async {
        loop {
            let snapshot: Value = server
                .client
                .get(format!("{}/admin/overview", server.url))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            if snapshot["queue"]["waiting"] == 1 {
                assert_eq!(snapshot["status"]["state"], "busy");
                break;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(!unload.is_finished());
    stream.text().await.unwrap();
    assert_eq!(unload.await.unwrap().status(), 200);
}

#[tokio::test]
async fn management_html_actions_report_results_and_reject_foreign_origins() {
    let server = Server::start("").await;
    let rejected = server
        .client
        .post(format!("{}/admin/ui/models/llama/load", server.url))
        .header("origin", "https://unrelated.example")
        .send()
        .await
        .unwrap();
    assert_eq!(rejected.status(), 403);
    assert!(server.events().is_empty());
    let loaded = server
        .client
        .post(format!("{}/admin/ui/models/llama/load", server.url))
        .send()
        .await
        .unwrap();
    assert_eq!(loaded.status(), 200);
    assert_eq!(loaded.headers()["hx-trigger"], "refresh");
    assert!(
        loaded
            .text()
            .await
            .unwrap()
            .contains("ロードが完了しました")
    );
    let html = server
        .client
        .get(format!("{}/admin/ui/dashboard", server.url))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(html.contains("ロード済み"));
    let missing = server
        .client
        .post(format!("{}/admin/ui/models/missing/load", server.url))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 404);
    assert!(missing.text().await.unwrap().contains("操作に失敗しました"));
    let unloaded = server
        .client
        .post(format!("{}/admin/ui/unload", server.url))
        .send()
        .await
        .unwrap();
    assert_eq!(unloaded.status(), 200);
    assert!(
        unloaded
            .text()
            .await
            .unwrap()
            .contains("アンロードが完了しました")
    );
}
