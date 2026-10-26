use std::{fs, os::unix::fs::PermissionsExt, process::Command};

#[test]
fn strata_engine_is_resolved_from_runtime_path_before_startup() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let engine = bin.join("strata");
    fs::write(&engine, "#!/bin/sh\nexit 0\n").unwrap();
    fs::write(
        dir.path().join("lllmmm.toml"),
        "[[models]]\nid = 'strata'\nbackend = 'strata'\nexecutable = 'strata-server'\nport = 18096\nconfig_file = 'strata.json'\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("strata.json"),
        r#"{"exe":"strata","tokenizer":".","args":[]}"#,
    )
    .unwrap();
    let check = || {
        Command::new(env!("CARGO_BIN_EXE_lllmmm"))
            .args(["--config"])
            .arg(dir.path().join("lllmmm.toml"))
            .arg("check")
            .env("PATH", &bin)
            .output()
            .unwrap()
    };
    fs::set_permissions(&engine, fs::Permissions::from_mode(0o700)).unwrap();
    let output = check();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::set_permissions(&engine, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(
        !check().status.success(),
        "accepted a non-executable engine"
    );
    fs::remove_file(&engine).unwrap();
    assert!(!check().status.success(), "accepted a missing engine");
}
