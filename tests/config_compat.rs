use asr33emu::core::config::{ConfigCli, LoadedConfig};
use clap::Parser;
use serde_json::{json, Value};
use std::path::Path;

fn repository_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn frozen_legacy_snapshot(strict: bool) -> Value {
    let mut snapshot = json!({
        "sound": {
            "config": {
                "lid": "up",
                "mute_state": "unmuted"
            }
        },
        "terminal": {
            "config": {
                "mode": "line",
                "columns": 72,
                "rows": 24,
                "scrollback": 200,
                "autowrap": true,
                "keyboard_uppercase_only": false,
                "keyboard_parity_mode": "space",
                "send_cr_at_startup": false,
                "no_print": false,
                "font_path": null,
                "font_size": 20
            }
        },
        "backend": {
            "serial_config": {
                "port": "COM4",
                "baudrate": 19200,
                "databits": 8,
                "parity": "N",
                "stopbits": 1
            }
        },
        "data_throttle": {
            "config": {
                "mode": "throttled",
                "send_rate_cps": 10,
                "receive_rate_cps": 10
            }
        },
        "tape_reader": {
            "config": {
                "max_rows": 200,
                "initial_file_path": ".",
                "skip_leading_nulls": true,
                "auto_stop": true,
                "set_msb": false,
                "ghost_outline": true,
                "bit_label_base": 1,
                "ascii_char_mask_msb": true
            }
        },
        "tape_punch": {
            "config": {
                "max_rows": 200,
                "initial_file_path": ".",
                "mode": "overwrite",
                "ghost_outline": true,
                "bit_label_base": 1,
                "ascii_char_mask_msb": true
            }
        }
    });

    if strict {
        snapshot["terminal"]["config"]["autowrap"] = json!(false);
        snapshot["terminal"]["config"]["keyboard_uppercase_only"] = json!(true);
        snapshot["terminal"]["config"]["keyboard_parity_mode"] = json!("mark");
        snapshot["backend"]["serial_config"]["baudrate"] = json!(110);
    }

    snapshot
}

fn compare_rust_to_frozen_legacy(
    config_filename: &str,
    overrides: &[&str],
    expected: Value,
) {
    let config_path = repository_root().join(config_filename);
    let mut rust_args = vec!["asr33emu".to_owned(), "--config".to_owned()];
    rust_args.push(config_path.to_string_lossy().into_owned());
    rust_args.extend(overrides.iter().map(|argument| (*argument).to_owned()));

    let cli = ConfigCli::try_parse_from(rust_args).expect("compatibility CLI case is valid");
    let loaded = LoadedConfig::load(&cli).expect("compatibility YAML is valid");
    let mut rust_value =
        serde_json::to_value(&loaded.effective).expect("typed Rust config serializes to JSON");

    // `input_return_mode` was introduced by the Rust implementation after the
    // legacy behaviour was frozen. Everything else remains locked to the last
    // characterized configuration surface from the removed implementation.
    rust_value["terminal"]["config"]
        .as_object_mut()
        .expect("terminal config is an object")
        .remove("input_return_mode");

    assert_eq!(rust_value, expected);
}

#[test]
fn default_yaml_matches_frozen_legacy_snapshot() {
    compare_rust_to_frozen_legacy("asr33_config.yaml", &[], frozen_legacy_snapshot(false));
}

#[test]
fn strict_yaml_matches_frozen_legacy_snapshot() {
    compare_rust_to_frozen_legacy("asr33_strict.yaml", &[], frozen_legacy_snapshot(true));
}

#[test]
fn default_yaml_with_all_shared_cli_overrides_matches_frozen_legacy_snapshot() {
    let mut expected = frozen_legacy_snapshot(false);
    expected["terminal"]["config"]["mode"] = json!("local");
    expected["terminal"]["config"]["columns"] = json!(80);
    expected["terminal"]["config"]["rows"] = json!(30);
    expected["terminal"]["config"]["scrollback"] = json!(400);
    expected["data_throttle"]["config"]["send_rate_cps"] = json!(37);
    expected["data_throttle"]["config"]["receive_rate_cps"] = json!(37);
    expected["sound"]["config"]["mute_state"] = json!("muted");
    expected["backend"]["serial_config"]["baudrate"] = json!(9600);
    expected["backend"]["serial_config"]["databits"] = json!(7);
    expected["backend"]["serial_config"]["parity"] = json!("E");
    expected["backend"]["serial_config"]["stopbits"] = json!(2);

    compare_rust_to_frozen_legacy(
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
        expected,
    );
}

#[test]
fn strict_yaml_with_baud_alias_and_terminal_overrides_matches_frozen_legacy_snapshot() {
    let mut expected = frozen_legacy_snapshot(true);
    expected["terminal"]["config"]["columns"] = json!(81);
    expected["terminal"]["config"]["rows"] = json!(25);
    expected["terminal"]["config"]["scrollback"] = json!(201);
    expected["data_throttle"]["config"]["send_rate_cps"] = json!(0);
    expected["data_throttle"]["config"]["receive_rate_cps"] = json!(0);
    expected["backend"]["serial_config"]["baudrate"] = json!(19200);
    expected["backend"]["serial_config"]["databits"] = json!(5);
    expected["backend"]["serial_config"]["parity"] = json!("M");
    expected["backend"]["serial_config"]["stopbits"] = json!(1);

    compare_rust_to_frozen_legacy(
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
        expected,
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

    // Intentional improvement over the historical shallow-copy behaviour: the
    // parsed file value remains unchanged after CLI overrides are applied.
    assert_eq!(loaded.file.terminal.config.columns, 72);
    assert_eq!(loaded.effective.terminal.config.columns, 80);
}
