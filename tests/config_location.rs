use std::{fs, process::Command};

#[test]
fn config_location_uses_xdg_home_fallback_and_explicit_override() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let xdg = dir.path().join("custom config");
    for base in [home.join(".config"), xdg.clone()] {
        let config = base.join("lllmmm");
        fs::create_dir_all(&config).unwrap();
        fs::write(config.join("model.gguf"), "fixture").unwrap();
        fs::write(
            config.join("lllmmm.toml"),
            r#"
[[models]]
id = "fixture"
backend = "llama_cpp"
executable = "unused"
port = 18081
model_file = "model.gguf"
context_size = 32
"#,
        )
        .unwrap();
    }
    for value in [None, Some(""), Some("relative"), xdg.to_str()] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_lllmmm"));
        command
            .current_dir(dir.path())
            .env("HOME", &home)
            .env_remove("XDG_CONFIG_HOME")
            .arg("check");
        if let Some(value) = value {
            command.env("XDG_CONFIG_HOME", value);
        }
        let result = command.output().unwrap();
        assert!(
            result.status.success(),
            "{value:?}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    fs::remove_file(home.join(".config/lllmmm/lllmmm.toml")).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_lllmmm"))
        .env_remove("HOME")
        .env("XDG_CONFIG_HOME", "relative")
        .args([
            "--config",
            xdg.join("lllmmm/lllmmm.toml").to_str().unwrap(),
            "check",
        ])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let result = Command::new(env!("CARGO_BIN_EXE_lllmmm"))
        .env("XDG_CONFIG_HOME", &xdg)
        .env_remove("HOME")
        .arg("check")
        .output()
        .unwrap();
    assert!(result.status.success());
    let result = Command::new(env!("CARGO_BIN_EXE_lllmmm"))
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("HOME")
        .arg("check")
        .output()
        .unwrap();
    assert!(!result.status.success());
}
