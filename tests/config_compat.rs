use asr33emu::core::config::{ConfigCli, LoadedConfig};
use clap::Parser;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repository_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn compare_python_and_rust(config_filename: &str, overrides: &[&str]) {
    let config_path = repository_root().join(config_filename);
    let mut rust_args = vec!["asr33emu".to_owned(), "--config".to_owned()];
    rust_args.push(config_path.to_string_lossy().into_owned());
    rust_args.extend(overrides.iter().map(|argument| (*argument).to_owned()));

    let cli = ConfigCli::try_parse_from(rust_args).expect("shared CLI case is valid in Rust");
    let loaded = LoadedConfig::load(&cli).expect("shared YAML is valid in Rust");
    let mut rust_value =
        serde_json::to_value(&loaded.effective).expect("typed Rust config serializes to JSON");
    // These Rust-only, serde-defaulted options are intentionally absent from
    // the unchanged legacy Python model; every remaining legacy field stays
    // differential. Backend selection is intentionally no longer part of the
    // Rust surface and therefore is not exercised as a shared override.
    rust_value["terminal"]["config"]
        .as_object_mut()
        .expect("terminal config is an object")
        .remove("input_return_mode");

    let python = std::env::var_os("PYTHON")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("python"));
    let mut command = Command::new(python);
    command
        .current_dir(repository_root())
        .arg(repository_root().join("tests/python_config_snapshot.py"))
        .arg("--config")
        .arg(&config_path)
        .args(overrides);
    let output = command
        .output()
        .expect("Python interpreter runs the differential helper");
    assert!(
        output.status.success(),
        "Python config helper failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let python_value: Value =
        serde_json::from_slice(&output.stdout).expect("Python helper emits valid JSON");

    assert_eq!(rust_value, python_value);
}

#[test]
fn default_yaml_matches_python_without_overrides() {
    compare_python_and_rust("asr33_config.yaml", &[]);
}

#[test]
fn strict_yaml_matches_python_without_overrides() {
    compare_python_and_rust("asr33_strict.yaml", &[]);
}

#[test]
fn default_yaml_with_all_shared_cli_overrides_matches_python() {
    compare_python_and_rust(
        "asr33_config.yaml",
        &[
            "--term_mode",
            "local",
            "--columns",
            "80",
            "--rows",
            "30",
            "--scrollback",
            "400",
            "--throttle_rate",
            "37",
            "--mute",
            "--baudrate",
            "9600",
            "--databits",
            "7",
            "--parity",
            "E",
            "--stopbits",
            "2",
        ],
    );
}

#[test]
fn strict_yaml_with_baud_alias_and_terminal_overrides_matches_python() {
    compare_python_and_rust(
        "asr33_strict.yaml",
        &[
            "--term_mode",
            "line",
            "--columns",
            "81",
            "--rows",
            "25",
            "--scrollback",
            "201",
            "--throttle_rate",
            "0",
            "--baud",
            "19200",
            "--databits",
            "5",
            "--parity",
            "M",
            "--stopbits",
            "1",
        ],
    );
}

#[test]
fn rust_cli_rejects_backend_selection() {
    let path = repository_root().join("asr33_config.yaml");
    let result = ConfigCli::try_parse_from([
        "asr33emu",
        "--config",
        path.to_str().expect("repository path is UTF-8"),
        "--backend",
        "serial",
    ]);
    assert!(result.is_err(), "backend selection is intentionally gone");
}

#[test]
fn rust_keeps_file_and_effective_configuration_independent() {
    let path = repository_root().join("asr33_config.yaml");
    let cli = ConfigCli::try_parse_from([
        "asr33emu",
        "--config",
        path.to_str().expect("repository path is UTF-8"),
        "--columns",
        "80",
    ])
    .expect("test CLI is valid");
    let loaded = LoadedConfig::load(&cli).expect("default YAML is valid");

    // Intentional difference from Python's characterized shallow-copy legacy
    // behavior: the parsed file value remains unchanged.
    assert_eq!(loaded.file.terminal.config.columns, 72);
    assert_eq!(loaded.effective.terminal.config.columns, 80);
}
