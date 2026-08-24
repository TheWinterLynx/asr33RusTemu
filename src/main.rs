use std::error::Error;
use std::io;
use std::process::ExitCode;

use asr33emu::adapters::transport::serial::SerialTransport;
use asr33emu::app::{AppRuntime, SystemScheduler};
use asr33emu::core::config::{
    AppConfig, ConfigCli, LoadedConfig, TerminalMode, ThrottleMode as ConfigThrottleMode,
};
use asr33emu::core::events::{ApplicationCommand, CommunicationMode, ThrottleMode};
use asr33emu::core::terminal::TerminalOptions;
use asr33emu::core::throttle::ThrottleConfig;
use asr33emu::ui::keyboard::KeyboardOptions;
use asr33emu::ui::{EguiApp, UiOptions};
use clap::Parser;

const DEFAULT_CONFIG_PATH: &str = "asr33_config.yaml";
const EMBEDDED_DEFAULT_CONFIG: &str = include_str!("../asr33_config.yaml");

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("asr33emu: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let cli = ConfigCli::parse();
    let loaded = load_startup_config(&cli)?;
    let config = loaded.effective;
    config.validate()?;

    let terminal_config = &config.terminal.config;
    let terminal_options = terminal_options(&config);
    let throttle_config = throttle_config(&config);
    let (communication_mode, throttle_mode) = configured_modes(&config);
    let printer_enabled = !terminal_config.no_print;
    let keyboard = KeyboardOptions {
        uppercase_only: terminal_config.keyboard_uppercase_only,
        parity: terminal_config.keyboard_parity_mode,
        return_mode: terminal_config.input_return_mode,
    };
    let tape_reader = config.tape_reader.config.clone();
    let tape_punch = config.tape_punch.config.clone();
    let font_size = terminal_config.font_size as f32;
    let initial_commands =
        initial_commands(&config, communication_mode, throttle_mode, printer_enabled);

    let serial_config = config.backend.serial_config.clone();
    // Deliberate policy: configuration describes the desired serial port, but
    // startup never opens it. A physical/virtual COM port is opened only after
    // the user explicitly presses Connect. This prevents a missing or busy port
    // from making the emulator itself fail to start.
    let mut runtime = AppRuntime::<SerialTransport, _>::new_disconnected(
        SystemScheduler::new(),
        terminal_options,
        throttle_config,
    )?;
    runtime.start_with_initial_commands(initial_commands)?;
    runtime.configure_startup_cr(config.terminal.config.send_cr_at_startup);

    let ui_options = UiOptions {
        title: "ASR-33 Teletype Emulator".to_owned(),
        backend_label: "serial".to_owned(),
        font_size,
        keyboard,
        communication_mode,
        throttle_mode,
        printer_enabled,
        tape_reader,
        tape_punch,
        serial_config,
        config_path: cli.config,
        disk_config: loaded.file,
        applied_config: config,
    };
    let title = ui_options.title.clone();
    let native_options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 780.0])
            .with_min_inner_size([720.0, 480.0])
            .with_resizable(true),
        ..Default::default()
    };
    eframe::run_native(
        &title,
        native_options,
        Box::new(move |context| Ok(Box::new(EguiApp::new(context, runtime, ui_options)))),
    )
    .map_err(|error| io::Error::other(format!("eframe failed: {error}")))?;
    Ok(())
}

fn load_startup_config(cli: &ConfigCli) -> Result<LoadedConfig, Box<dyn Error>> {
    let default_path = std::path::PathBuf::from(DEFAULT_CONFIG_PATH);
    if cli.config == default_path && !cli.config.exists() {
        return embedded_default_config(cli);
    }
    Ok(LoadedConfig::load(cli)?)
}

fn embedded_default_config(cli: &ConfigCli) -> Result<LoadedConfig, Box<dyn Error>> {
    let file = AppConfig::from_yaml_str(EMBEDDED_DEFAULT_CONFIG)?;
    let mut effective = file.clone();
    cli.apply_to(&mut effective);
    effective.validate()?;
    Ok(LoadedConfig { file, effective })
}

fn terminal_options(config: &AppConfig) -> TerminalOptions {
    let terminal = &config.terminal.config;
    TerminalOptions {
        columns: terminal.columns,
        rows: terminal.rows,
        scrollback: terminal.scrollback,
        autowrap: terminal.autowrap,
    }
}

fn throttle_config(config: &AppConfig) -> ThrottleConfig {
    ThrottleConfig {
        tx_rate_cps: config.data_throttle.config.send_rate_cps,
        rx_rate_cps: config.data_throttle.config.receive_rate_cps,
        ..ThrottleConfig::default()
    }
}

fn configured_modes(config: &AppConfig) -> (CommunicationMode, ThrottleMode) {
    let communication = match config.terminal.config.mode {
        TerminalMode::Line => CommunicationMode::Line,
        TerminalMode::Local => CommunicationMode::Local,
    };
    let throttle = match config.data_throttle.config.mode {
        ConfigThrottleMode::Throttled => ThrottleMode::Throttled,
        ConfigThrottleMode::Unthrottled => ThrottleMode::Unthrottled,
    };
    (communication, throttle)
}

fn initial_commands(
    config: &AppConfig,
    communication_mode: CommunicationMode,
    throttle_mode: ThrottleMode,
    printer_enabled: bool,
) -> Vec<ApplicationCommand> {
    let mut commands = Vec::new();
    commands.extend([
        ApplicationCommand::SetCommunicationMode(communication_mode),
        ApplicationCommand::SetThrottleMode(throttle_mode),
        ApplicationCommand::SetTxRate(config.data_throttle.config.send_rate_cps),
        ApplicationCommand::SetRxRate(config.data_throttle.config.receive_rate_cps),
        ApplicationCommand::SetPrinterEnabled(printer_enabled),
    ]);
    commands
}

#[cfg(test)]
mod tests {
    use super::{
        configured_modes, embedded_default_config, initial_commands, terminal_options,
        throttle_config,
    };
    use asr33emu::core::config::{AppConfig, ConfigCli, KeyboardParityMode};
    use asr33emu::core::events::{ApplicationCommand, CommunicationMode, ThrottleMode};
    use clap::Parser;

    #[test]
    fn default_yaml_maps_to_runtime_dimensions_rates_and_modes() {
        let config = AppConfig::from_yaml_str(include_str!("../asr33_config.yaml"))
            .expect("bundled YAML remains valid");
        let terminal = terminal_options(&config);
        assert_eq!(
            (terminal.columns, terminal.rows, terminal.scrollback),
            (72, 24, 200)
        );
        assert!(terminal.autowrap);
        let throttle = throttle_config(&config);
        assert_eq!((throttle.tx_rate_cps, throttle.rx_rate_cps), (10, 10));
        assert_eq!(
            configured_modes(&config),
            (CommunicationMode::Line, ThrottleMode::Throttled)
        );
    }

    #[test]
    fn embedded_default_config_accepts_cli_overrides_without_disk_file() {
        let cli = ConfigCli::try_parse_from([
            "asr33emu",
            "--columns",
            "80",
            "--baud",
            "9600",
            "--mute",
        ])
        .expect("CLI is valid");
        let loaded = embedded_default_config(&cli).expect("embedded default config loads");
        assert_eq!(loaded.file.terminal.config.columns, 72);
        assert_eq!(loaded.effective.terminal.config.columns, 80);
        assert_eq!(loaded.effective.backend.serial_config.baudrate, 9600);
        assert!(matches!(
            loaded.effective.sound.config.mute_state,
            asr33emu::core::config::MuteState::Muted
        ));
    }

    #[test]
    fn startup_cr_is_not_encoded_or_buffered_as_an_initial_keyboard_command() {
        for parity in [
            KeyboardParityMode::Space,
            KeyboardParityMode::Mark,
            KeyboardParityMode::Even,
        ] {
            let mut config = AppConfig::from_yaml_str(include_str!("../asr33_config.yaml"))
                .expect("bundled YAML remains valid");
            config.terminal.config.send_cr_at_startup = true;
            config.terminal.config.keyboard_parity_mode = parity;
            let commands = initial_commands(
                &config,
                CommunicationMode::Local,
                ThrottleMode::Unthrottled,
                false,
            );
            assert_eq!(
                commands,
                [
                    ApplicationCommand::SetCommunicationMode(CommunicationMode::Local),
                    ApplicationCommand::SetThrottleMode(ThrottleMode::Unthrottled),
                    ApplicationCommand::SetTxRate(10),
                    ApplicationCommand::SetRxRate(10),
                    ApplicationCommand::SetPrinterEnabled(false),
                ],
                "startup CR is now a session-scoped connection action, independent of {parity:?}"
            );
        }
    }
}
