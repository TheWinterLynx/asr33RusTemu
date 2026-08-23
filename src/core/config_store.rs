//! Typed, recoverable persistence for [`AppConfig`](super::config::AppConfig).

use std::error::Error;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use super::config::{AppConfig, ConfigError, ValidationError};

#[derive(Clone, Debug)]
pub struct ConfigStore {
    path: PathBuf,
}

impl ConfigStore {
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> Result<AppConfig, ConfigStoreError> {
        let source = fs::read_to_string(&self.path).map_err(|source| ConfigStoreError::Io {
            path: self.path.clone(),
            source,
        })?;
        AppConfig::from_yaml_str(&source).map_err(ConfigStoreError::Parse)
    }

    pub fn save(&self, config: &AppConfig) -> Result<(), ConfigStoreError> {
        config.validate().map_err(ConfigStoreError::Validation)?;
        let yaml = config
            .to_yaml_string()
            .map_err(ConfigStoreError::Serialize)?;
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        let name = self
            .path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("config.yaml");
        let temporary = parent.join(format!(".{name}.{}.tmp", std::process::id()));
        let result = (|| {
            let mut file = File::create(&temporary).map_err(|source| ConfigStoreError::Io {
                path: temporary.clone(),
                source,
            })?;
            file.write_all(yaml.as_bytes())
                .map_err(|source| ConfigStoreError::Io {
                    path: temporary.clone(),
                    source,
                })?;
            file.sync_all().map_err(|source| ConfigStoreError::Io {
                path: temporary.clone(),
                source,
            })?;
            replace_file(&temporary, &self.path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}

#[cfg(not(windows))]
fn replace_file(temporary: &Path, target: &Path) -> Result<(), ConfigStoreError> {
    fs::rename(temporary, target).map_err(|source| ConfigStoreError::Io {
        path: target.to_path_buf(),
        source,
    })
}

#[cfg(windows)]
fn replace_file(temporary: &Path, target: &Path) -> Result<(), ConfigStoreError> {
    if !target.exists() {
        return fs::rename(temporary, target).map_err(|source| ConfigStoreError::Io {
            path: target.to_path_buf(),
            source,
        });
    }
    let backup = target.with_extension(format!(
        "{}.settings-backup",
        target
            .extension()
            .and_then(|x| x.to_str())
            .unwrap_or("yaml")
    ));
    fs::rename(target, &backup).map_err(|source| ConfigStoreError::Io {
        path: target.to_path_buf(),
        source,
    })?;
    if let Err(source) = fs::rename(temporary, target) {
        let _ = fs::rename(&backup, target);
        return Err(ConfigStoreError::Io {
            path: target.to_path_buf(),
            source,
        });
    }
    fs::remove_file(&backup).map_err(|source| ConfigStoreError::Io {
        path: backup,
        source,
    })
}

#[derive(Debug)]
pub enum ConfigStoreError {
    Io { path: PathBuf, source: io::Error },
    Parse(ConfigError),
    Serialize(serde_saphyr::SerializeError),
    Validation(ValidationError),
}

impl fmt::Display for ConfigStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(f, "configuration I/O at {}: {source}", path.display())
            }
            Self::Parse(error) => write!(f, "configuration parse failed: {error}"),
            Self::Serialize(error) => write!(f, "configuration serialization failed: {error}"),
            Self::Validation(error) => write!(f, "configuration validation failed: {error}"),
        }
    }
}
impl Error for ConfigStoreError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::AppConfig;

    fn sample() -> AppConfig {
        AppConfig::from_yaml_str(include_str!("../../asr33_config.yaml")).expect("fixture")
    }

    #[test]
    fn round_trip_preserves_complete_typed_config_and_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("selected.yaml");
        let store = ConfigStore::new(path.clone());
        let mut config = sample();
        config.terminal.config.columns = 81;
        store.save(&config).expect("save");
        assert_eq!(store.path(), path);
        assert_eq!(store.load().expect("load"), config);
    }

    #[test]
    fn loads_repository_yaml_through_typed_store() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("asr33_config.yaml");
        let loaded = ConfigStore::new(path).load().expect("load repository YAML");
        assert_eq!(loaded, sample());
    }

    #[test]
    fn invalid_config_does_not_overwrite_existing_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("selected.yaml");
        fs::write(&path, "sentinel").expect("fixture write");
        let mut config = sample();
        config.terminal.config.columns = 0;
        let error = ConfigStore::new(path.clone())
            .save(&config)
            .expect_err("invalid");
        assert!(matches!(error, ConfigStoreError::Validation(_)));
        assert_eq!(fs::read_to_string(path).expect("read"), "sentinel");
    }

    #[test]
    fn write_failure_is_typed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = ConfigStore::new(dir.path().to_path_buf());
        assert!(matches!(
            store.save(&sample()),
            Err(ConfigStoreError::Io { .. })
        ));
    }
}
