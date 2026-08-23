use std::error::Error;
use std::io;
use std::process::ExitCode;

use asr33emu::adapters::transport::serial::SerialTransport;
use asr33emu::app::{AppRuntime, SystemScheduler};
use asr33emu::core::config::{
    AppConfig, BackendKind, ConfigCli, LoadedConfig, TerminalMode,
    ThrottleMode as ConfigThrottleMode,
};
use asr33emu::core::events::{ApplicationCommand, CommunicationMode, ThrottleMode};
use asr33emu::core::terminal::TerminalOptions;
use asr33emu::core::throttle::ThrottleConfig;
use asr33emu::ui::keyboard::KeyboardOptions;
use asr33emu::ui::{EguiApp, UiOptions};
use clap::Parser;

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
    let loaded = LoadedConfig::load(&cli)?;
    let config = loaded.effective;
    config.validate()?;

    if config.backend.kind == BackendKind::Ssh {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "the SSH backend has not been migrated to Rust yet",
        )
        .into());
    }

    let terminal_config = &config.terminal.config;
    let terminal_options = terminal_options(&config);
    let throttle_config = throttle_config(&config);
    let (communication_mode, throttle_mode) = configured_modes(&config);
    let printer_enabled = !terminal_config.no_print;
    let keyboard = KeyboardOptions {
        uppercase_only: terminal_config.keyboard_uppercase_only,
        parity: terminal_config.keyboard_parity_mode,
        return_mode: terminal_config.keyboard_return_mode,
    };
    let tape_reader = config.tape_reader.config.clone();
    let tape_punch = config.tape_punch.config.clone();
    let font_size = terminal_config.font_size as f32;
    let backend_label = "serial".to_owned();
    let initial_commands =
        initial_commands(&config, communication_mode, throttle_mode, printer_enabled);

    let serial_config = config.backend.serial_config.clone();
    let mut runtime = AppRuntime::<SerialTransport, _>::new_disconnected(
        SystemScheduler::new(),
        terminal_options,
        throttle_config,
    )?;
    runtime.start_with_initial_commands(initial_commands)?;
    runtime.configure_startup_cr(config.terminal.config.send_cr_at_startup);

    let ui_options = UiOptions {
        title: "ASR-33 Teletype Emulator".to_owned(),
        backend_label,
        font_size,
        keyboard,
        communication_mode,
        throttle_mode,
        printer_enabled,
        tape_reader,
        tape_punch,
        serial_config,
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
    use super::{configured_modes, initial_commands, terminal_options, throttle_config};
    use asr33emu::core::config::{AppConfig, KeyboardParityMode};
    use asr33emu::core::events::{ApplicationCommand, CommunicationMode, ThrottleMode};

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
