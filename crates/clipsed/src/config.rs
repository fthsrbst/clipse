//! On-disk daemon configuration.
//!
//! Holds the device identity (generated once, then stable forever — paired
//! peers key on it) and the user-editable settings that the UI round-trips
//! over IPC.

use std::path::Path;

use clipse_core::{DeviceId, Paths};
use clipse_ipc::Settings;
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("reading {path}: {source}")]
    Read { path: String, source: std::io::Error },

    #[error("writing {path}: {source}")]
    Write { path: String, source: std::io::Error },

    #[error("{path} is not valid TOML: {source}")]
    Parse { path: String, source: toml::de::Error },

    #[error("could not serialise config: {0}")]
    Serialise(#[from] toml::ser::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Stable for the life of the installation. Changing it un-pairs every
    /// peer, so it is never regenerated once written.
    pub device: DeviceId,

    #[serde(default)]
    pub settings: Settings,
}

impl Config {
    /// Load the config, creating it with a fresh device identity on first run.
    pub fn load_or_create(paths: &Paths) -> Result<Self, ConfigError> {
        let path = paths.config();

        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).map_err(|source| ConfigError::Parse {
                path: path.display().to_string(),
                source,
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let config = Self { device: DeviceId::generate(), settings: Settings::default() };
                config.save(paths)?;
                Ok(config)
            }
            Err(source) => Err(ConfigError::Read { path: path.display().to_string(), source }),
        }
    }

    pub fn save(&self, paths: &Paths) -> Result<(), ConfigError> {
        let text = toml::to_string_pretty(self)?;
        write_atomically(&paths.config(), text.as_bytes())
    }
}

/// Write via a sibling temp file and rename. A half-written config would cost
/// the user their device identity, and with it every pairing.
fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), ConfigError> {
    let err = |source: std::io::Error| ConfigError::Write { path: path.display().to_string(), source };

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(err)?;
    }

    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, bytes).map_err(err)?;
    std::fs::rename(&tmp, path).map_err(err)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_run_generates_a_device_and_persists_it() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(dir.path());

        let first = Config::load_or_create(&paths).unwrap();
        let second = Config::load_or_create(&paths).unwrap();

        assert_eq!(first.device, second.device, "device identity must be stable");
        assert!(paths.config().exists());
    }

    #[test]
    fn settings_survive_a_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(dir.path());

        let mut config = Config::load_or_create(&paths).unwrap();
        config.settings.hotkey = "Alt+Space".into();
        config.settings.apply_incoming_to_clipboard = false;
        config.settings.blocked_apps = vec!["1Password".into()];
        config.save(&paths).unwrap();

        let reloaded = Config::load_or_create(&paths).unwrap();
        assert_eq!(reloaded.settings, config.settings);
    }

    #[test]
    fn a_config_missing_optional_fields_still_loads() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(dir.path());
        paths.create_all().unwrap();

        let device = DeviceId::generate();
        std::fs::write(paths.config(), format!("device = \"{device}\"\n")).unwrap();

        let config = Config::load_or_create(&paths).unwrap();
        assert_eq!(config.device, device);
        assert!(config.settings.detect_secrets, "defaults must fill in");
    }

    #[test]
    fn corrupt_config_fails_loudly_rather_than_resetting_identity() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(dir.path());
        paths.create_all().unwrap();
        std::fs::write(paths.config(), "this is not toml = = =").unwrap();

        let err = Config::load_or_create(&paths).unwrap_err();
        assert!(matches!(err, ConfigError::Parse { .. }), "{err}");
    }
}
