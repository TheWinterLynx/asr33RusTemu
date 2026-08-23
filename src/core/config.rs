//! Strongly typed YAML configuration and legacy-compatible CLI overrides.
//!
//! Python's `dict.copy()` merge mutates nested values in both its raw and
//! effective trees. That legacy behavior is characterized by Python tests but
//! is intentionally not reproduced here: [`LoadedConfig::file`] is the parsed
//! file and [`LoadedConfig::effective`] is an independently owned clone with
//! CLI overrides. This difference must be approved before Rust configuration
//! is connected to the application.

use clap::{Parser, ValueEnum};
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::PathBuf;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AppConfig {
    pub frontend: FrontendSection,
    pub sound: SoundSection,
    pub terminal: TerminalSection,
    pub backend: BackendSection,
    pub data_throttle: DataThrottleSection,
    pub tape_reader: TapeReaderSection,
    pub tape_punch: TapePunchSection,
}

impl AppConfig {
    pub fn from_yaml_str(source: &str) -> Result<Self, ConfigError> {
        serde_saphyr::from_str(source).map_err(ConfigError::Yaml)
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        let terminal = &self.terminal.config;
        require_nonzero(terminal.columns, "terminal.config.columns")?;
        require_nonzero(terminal.rows, "terminal.config.rows")?;
        terminal.rows.checked_add(terminal.scrollback).ok_or(
            ValidationError::CapacityOverflow {
                field: "terminal.config.rows + terminal.config.scrollback",
            },
        )?;
        require_nonzero(terminal.font_size, "terminal.config.font_size")?;

        match self.backend.kind {
            BackendKind::Serial => {
                let serial = &self.backend.serial_config;
                require_nonempty(&serial.port, "backend.serial_config.port")?;
                require_nonzero(serial.baudrate, "backend.serial_config.baudrate")?;
            }
            BackendKind::Ssh => {
                let ssh = &self.backend.ssh_config;
                require_nonempty(&ssh.username, "backend.ssh_config.username")?;
                require_nonempty(&ssh.host, "backend.ssh_config.host")?;
                require_nonzero(ssh.port, "backend.ssh_config.port")?;
            }
        }

        require_nonzero(
            self.tape_reader.config.max_rows,
            "tape_reader.config.max_rows",
        )?;
        require_nonzero(
            self.tape_punch.config.max_rows,
            "tape_punch.config.max_rows",
        )?;
        Ok(())
    }
}

fn require_nonzero<T>(value: T, field: &'static str) -> Result<(), ValidationError>
where
    T: Default + PartialEq,
{
    if value == T::default() {
        Err(ValidationError::MustBePositive { field })
    } else {
        Ok(())
    }
}

fn require_nonempty(value: &str, field: &'static str) -> Result<(), ValidationError> {
    if value.trim().is_empty() {
        Err(ValidationError::MustNotBeEmpty { field })
    } else {
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadedConfig {
    pub file: AppConfig,
    pub effective: AppConfig,
}

impl LoadedConfig {
    pub fn load(cli: &ConfigCli) -> Result<Self, ConfigError> {
        let source = fs::read_to_string(&cli.config).map_err(|source| ConfigError::Io {
            path: cli.config.clone(),
            source,
        })?;
        let file = AppConfig::from_yaml_str(&source)?;
        let mut effective = file.clone();
        cli.apply_to(&mut effective);
        effective.validate().map_err(ConfigError::Validation)?;
        Ok(Self { file, effective })
    }
}

#[derive(Clone, Debug, Parser)]
#[command(name = "asr33emu", about = "ASR-33 Teletype Emulator")]
pub struct ConfigCli {
    #[arg(long, default_value = "asr33_config.yaml")]
    pub config: PathBuf,

    #[arg(long, value_enum)]
    pub frontend: Option<FrontendConfigValue>,

    #[arg(long, value_enum)]
    pub backend: Option<BackendKind>,

    #[arg(long = "term_mode", value_enum)]
    pub term_mode: Option<TerminalMode>,

    #[arg(long)]
    pub columns: Option<usize>,

    #[arg(long)]
    pub rows: Option<usize>,

    #[arg(long)]
    pub scrollback: Option<usize>,

    #[arg(long = "throttle_rate")]
    pub throttle_rate: Option<i64>,

    #[arg(long, action = clap::ArgAction::SetTrue)]
    pub mute: bool,

    #[arg(long, alias = "baudrate")]
    pub baud: Option<u32>,

    #[arg(long, value_enum)]
    pub databits: Option<DataBits>,

    #[arg(long, value_enum, ignore_case = false)]
    pub parity: Option<SerialParity>,

    #[arg(long, value_enum)]
    pub stopbits: Option<StopBits>,
}

impl ConfigCli {
    pub fn apply_to(&self, config: &mut AppConfig) {
        if let Some(value) = self.frontend {
            config.frontend.kind = value;
        }
        if let Some(value) = self.backend {
            config.backend.kind = value;
        }
        if let Some(value) = self.term_mode {
            config.terminal.config.mode = value;
        }
        if let Some(value) = self.columns {
            config.terminal.config.columns = value;
        }
        if let Some(value) = self.rows {
            config.terminal.config.rows = value;
        }
        if let Some(value) = self.scrollback {
            config.terminal.config.scrollback = value;
        }
        if let Some(value) = self.throttle_rate {
            config.data_throttle.config.send_rate_cps = value;
            config.data_throttle.config.receive_rate_cps = value;
        }
        if self.mute {
            config.sound.config.mute_state = MuteState::Muted;
        }
        if let Some(value) = self.baud {
            config.backend.serial_config.baudrate = value;
        }
        if let Some(value) = self.databits {
            config.backend.serial_config.databits = value;
        }
        if let Some(value) = self.parity {
            config.backend.serial_config.parity = value;
        }
        if let Some(value) = self.stopbits {
            config.backend.serial_config.stopbits = value;
        }
    }
}

/// Values accepted from the legacy Python frontend setting.
///
/// These variants are compatibility inputs, not Rust UI architecture choices.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, ValueEnum)]
pub enum FrontendConfigValue {
    #[serde(rename = "tkinter")]
    #[value(name = "tkinter")]
    LegacyTkinter,
    #[serde(rename = "pygame")]
    #[value(name = "pygame")]
    LegacyPygame,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FrontendSection {
    #[serde(rename = "type")]
    pub kind: FrontendConfigValue,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LidState {
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MuteState {
    Muted,
    Unmuted,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SoundSection {
    pub config: SoundConfig,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SoundConfig {
    pub lid: LidState,
    pub mute_state: MuteState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum TerminalMode {
    Line,
    Local,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyboardParityMode {
    Mark,
    Space,
    Even,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyboardReturnMode {
    #[default]
    Cr,
    CrLf,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TerminalSection {
    pub config: TerminalConfig,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TerminalConfig {
    pub mode: TerminalMode,
    pub columns: usize,
    pub rows: usize,
    pub scrollback: usize,
    pub autowrap: bool,
    pub keyboard_uppercase_only: bool,
    pub keyboard_parity_mode: KeyboardParityMode,
    #[serde(default)]
    pub keyboard_return_mode: KeyboardReturnMode,
    pub send_cr_at_startup: bool,
    pub no_print: bool,
    pub font_path: Option<PathBuf>,
    pub font_size: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum BackendKind {
    Serial,
    Ssh,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BackendSection {
    #[serde(rename = "type")]
    pub kind: BackendKind,
    pub serial_config: SerialConfig,
    pub ssh_config: SshConfig,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum DataBits {
    #[value(name = "5")]
    Five,
    #[value(name = "6")]
    Six,
    #[value(name = "7")]
    Seven,
    #[value(name = "8")]
    Eight,
}

impl From<DataBits> for u8 {
    fn from(value: DataBits) -> Self {
        match value {
            DataBits::Five => 5,
            DataBits::Six => 6,
            DataBits::Seven => 7,
            DataBits::Eight => 8,
        }
    }
}

impl TryFrom<u8> for DataBits {
    type Error = NumericEnumError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            5 => Ok(Self::Five),
            6 => Ok(Self::Six),
            7 => Ok(Self::Seven),
            8 => Ok(Self::Eight),
            _ => Err(NumericEnumError::new("data bits", value)),
        }
    }
}

impl Serialize for DataBits {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_u8((*self).into())
    }
}

impl<'de> Deserialize<'de> for DataBits {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Self::try_from(u8::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, ValueEnum)]
pub enum SerialParity {
    #[serde(rename = "N")]
    #[value(name = "N")]
    None,
    #[serde(rename = "E")]
    #[value(name = "E")]
    Even,
    #[serde(rename = "O")]
    #[value(name = "O")]
    Odd,
    #[serde(rename = "M")]
    #[value(name = "M")]
    Mark,
    #[serde(rename = "S")]
    #[value(name = "S")]
    Space,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum StopBits {
    #[value(name = "1")]
    One,
    #[value(name = "1.5")]
    OnePointFive,
    #[value(name = "2")]
    Two,
}

impl Serialize for StopBits {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::One => serializer.serialize_u8(1),
            Self::OnePointFive => serializer.serialize_f64(1.5),
            Self::Two => serializer.serialize_u8(2),
        }
    }
}

impl<'de> Deserialize<'de> for StopBits {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct StopBitsVisitor;

        impl serde::de::Visitor<'_> for StopBitsVisitor {
            type Value = StopBits;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("1, 1.5, or 2 stop bits")
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                match value {
                    1 => Ok(StopBits::One),
                    2 => Ok(StopBits::Two),
                    _ => Err(E::custom(format!("unsupported stop bits: {value}"))),
                }
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                match value {
                    1 => Ok(StopBits::One),
                    2 => Ok(StopBits::Two),
                    _ => Err(E::custom(format!("unsupported stop bits: {value}"))),
                }
            }

            fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                if value == 1.0 {
                    Ok(StopBits::One)
                } else if value == 1.5 {
                    Ok(StopBits::OnePointFive)
                } else if value == 2.0 {
                    Ok(StopBits::Two)
                } else {
                    Err(E::custom(format!("unsupported stop bits: {value}")))
                }
            }
        }

        deserializer.deserialize_any(StopBitsVisitor)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SerialConfig {
    pub port: String,
    pub baudrate: u32,
    pub databits: DataBits,
    pub parity: SerialParity,
    pub stopbits: StopBits,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HostKeyPolicy {
    Strict,
    AcceptNew,
    Off,
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SshConfig {
    pub username: String,
    pub host: String,
    pub port: u16,
    pub key_filename: Option<PathBuf>,
    pub password: Option<String>,
    pub use_agent: bool,
    pub expected_fingerprint: Option<String>,
    pub host_key_policy: HostKeyPolicy,
    pub known_hosts_file: PathBuf,
    pub tofu_prompt: bool,
}

impl fmt::Debug for SshConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SshConfig")
            .field("username", &self.username)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("key_filename", &self.key_filename)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field("use_agent", &self.use_agent)
            .field("expected_fingerprint", &self.expected_fingerprint)
            .field("host_key_policy", &self.host_key_policy)
            .field("known_hosts_file", &self.known_hosts_file)
            .field("tofu_prompt", &self.tofu_prompt)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThrottleMode {
    Throttled,
    Unthrottled,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DataThrottleSection {
    pub config: DataThrottleConfig,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DataThrottleConfig {
    pub mode: ThrottleMode,
    pub send_rate_cps: i64,
    pub receive_rate_cps: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BitLabelBase {
    Zero,
    One,
}

impl From<BitLabelBase> for u8 {
    fn from(value: BitLabelBase) -> Self {
        match value {
            BitLabelBase::Zero => 0,
            BitLabelBase::One => 1,
        }
    }
}

impl TryFrom<u8> for BitLabelBase {
    type Error = NumericEnumError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Zero),
            1 => Ok(Self::One),
            _ => Err(NumericEnumError::new("bit label base", value)),
        }
    }
}

impl Serialize for BitLabelBase {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_u8((*self).into())
    }
}

impl<'de> Deserialize<'de> for BitLabelBase {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Self::try_from(u8::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TapeReaderSection {
    pub config: TapeReaderConfig,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TapeReaderConfig {
    pub max_rows: usize,
    pub initial_file_path: PathBuf,
    pub skip_leading_nulls: bool,
    pub auto_stop: bool,
    pub set_msb: bool,
    pub ghost_outline: bool,
    pub bit_label_base: BitLabelBase,
    pub ascii_char_mask_msb: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PunchConfigMode {
    Append,
    Overwrite,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TapePunchSection {
    pub config: TapePunchConfig,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TapePunchConfig {
    pub max_rows: usize,
    pub initial_file_path: PathBuf,
    pub mode: PunchConfigMode,
    pub ghost_outline: bool,
    pub bit_label_base: BitLabelBase,
    pub ascii_char_mask_msb: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NumericEnumError {
    kind: &'static str,
    value: u8,
}

impl NumericEnumError {
    const fn new(kind: &'static str, value: u8) -> Self {
        Self { kind, value }
    }
}

impl fmt::Display for NumericEnumError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid {} value: {}", self.kind, self.value)
    }
}

impl Error for NumericEnumError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ValidationError {
    MustBePositive { field: &'static str },
    MustNotBeEmpty { field: &'static str },
    CapacityOverflow { field: &'static str },
}

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MustBePositive { field } => {
                write!(formatter, "{field} must be greater than zero")
            }
            Self::MustNotBeEmpty { field } => write!(formatter, "{field} must not be empty"),
            Self::CapacityOverflow { field } => write!(formatter, "{field} overflows usize"),
        }
    }
}

impl Error for ValidationError {}

#[derive(Debug)]
pub enum ConfigError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Yaml(serde_saphyr::Error),
    Validation(ValidationError),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(formatter, "could not read {}: {source}", path.display())
            }
            Self::Yaml(source) => write!(formatter, "invalid YAML configuration: {source}"),
            Self::Validation(source) => write!(formatter, "invalid configuration: {source}"),
        }
    }
}

impl Error for ConfigError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Yaml(source) => Some(source),
            Self::Validation(source) => Some(source),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AppConfig, KeyboardReturnMode, StopBits, ValidationError};

    #[test]
    fn rejects_zero_terminal_columns() {
        let source = include_str!("../../asr33_config.yaml");
        let mut config = AppConfig::from_yaml_str(source).expect("repository YAML is valid");
        config.terminal.config.columns = 0;
        assert_eq!(
            config.validate(),
            Err(ValidationError::MustBePositive {
                field: "terminal.config.columns"
            })
        );
    }

    #[test]
    fn rejects_unknown_finite_state() {
        let source =
            include_str!("../../asr33_config.yaml").replace("mode: \"line\"", "mode: \"remote\"");
        assert!(AppConfig::from_yaml_str(&source).is_err());
    }

    #[test]
    fn debug_output_redacts_ssh_password() {
        let source = include_str!("../../asr33_config.yaml")
            .replace("password: null", "password: secret-value");
        let config = AppConfig::from_yaml_str(&source).expect("modified repository YAML is valid");
        let debug = format!("{:?}", config.backend.ssh_config);
        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains("secret-value"));
    }

    #[test]
    fn stop_bits_supports_one_point_five_without_changing_integer_yaml() {
        assert_eq!(
            serde_json::from_str::<StopBits>("1.5").expect("1.5 is supported"),
            StopBits::OnePointFive
        );
        assert_eq!(
            serde_json::to_string(&StopBits::One).expect("serializes"),
            "1"
        );
        assert_eq!(
            serde_json::to_string(&StopBits::Two).expect("serializes"),
            "2"
        );
    }

    #[test]
    fn existing_yaml_defaults_return_to_authentic_cr() {
        let config = AppConfig::from_yaml_str(include_str!("../../asr33_config.yaml"))
            .expect("repository YAML remains valid without the new key");
        assert_eq!(
            config.terminal.config.keyboard_return_mode,
            KeyboardReturnMode::Cr
        );
    }

    #[test]
    fn explicit_crlf_return_mode_roundtrips_through_yaml() {
        let source = include_str!("../../asr33_config.yaml").replace(
            "keyboard_parity_mode: \"space\"",
            "keyboard_parity_mode: \"space\"\n    keyboard_return_mode: \"crlf\"",
        );
        let config = AppConfig::from_yaml_str(&source).expect("explicit CRLF parses");
        assert_eq!(
            config.terminal.config.keyboard_return_mode,
            KeyboardReturnMode::CrLf
        );
        let serialized = serde_saphyr::to_string(&config).expect("configuration serializes");
        let roundtrip = AppConfig::from_yaml_str(&serialized).expect("serialized YAML parses");
        assert_eq!(
            roundtrip.terminal.config.keyboard_return_mode,
            KeyboardReturnMode::CrLf
        );
    }
}
