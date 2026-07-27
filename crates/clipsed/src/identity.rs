//! The device's long-term key and the set of devices it trusts, on disk.
//!
//! Separate from `config.toml` because this file is different in kind: losing
//! it un-pairs every device, and it is the one file in the data directory that
//! must never be shared, backed up carelessly, or copied to a second machine —
//! two installations with the same identity would be indistinguishable to
//! every peer.

use std::path::{Path, PathBuf};

use clipse_core::{DeviceId, Paths};
use clipse_crypto::{DeviceIdentity, Trust};
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    #[error("reading {path}: {source}")]
    Read {
        path: String,
        source: std::io::Error,
    },

    #[error("writing {path}: {source}")]
    Write {
        path: String,
        source: std::io::Error,
    },

    #[error("{path} is corrupt: {source}")]
    Parse {
        path: String,
        source: serde_json::Error,
    },

    #[error("could not serialise the identity: {0}")]
    Serialise(#[from] serde_json::Error),

    #[error("{path} belongs to device {stored}, but the config says {expected}")]
    Mismatch {
        path: String,
        stored: DeviceId,
        expected: DeviceId,
    },
}

#[derive(Deserialize)]
struct Stored {
    identity: DeviceIdentity,
    trust: Trust,
}

/// Borrowing rather than cloning: `Trust` is deliberately not `Clone`, so that
/// a second copy of the paired set cannot quietly drift from the real one.
#[derive(Serialize)]
struct StoredRef<'a> {
    identity: &'a DeviceIdentity,
    trust: &'a Trust,
}

/// The device key plus its paired set.
pub struct Identity {
    pub identity: DeviceIdentity,
    pub trust: Trust,
}

impl std::fmt::Debug for Identity {
    // Hand-written so this can never grow into something that prints the
    // secret half. `DeviceIdentity` already guards itself; this guards the
    // wrapper.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Identity")
            .field("device", &self.identity.device_id())
            .field("fingerprint", &self.identity.fingerprint())
            .field("epoch", &self.trust.epoch())
            .finish_non_exhaustive()
    }
}

impl Identity {
    fn file(paths: &Paths) -> PathBuf {
        paths.root().join("identity.json")
    }

    /// Load, or generate on first run.
    ///
    /// `device` comes from the config so the two files cannot drift apart: a
    /// mismatch means someone copied one file and not the other, which would
    /// leave the daemon presenting one identity and signing with another.
    pub fn load_or_create(paths: &Paths, device: DeviceId) -> Result<Self, IdentityError> {
        let path = Self::file(paths);

        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let stored: Stored =
                    serde_json::from_str(&text).map_err(|source| IdentityError::Parse {
                        path: path.display().to_string(),
                        source,
                    })?;
                if stored.identity.device_id() != device {
                    return Err(IdentityError::Mismatch {
                        path: path.display().to_string(),
                        stored: stored.identity.device_id(),
                        expected: device,
                    });
                }
                Ok(Self {
                    identity: stored.identity,
                    trust: stored.trust,
                })
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let identity = DeviceIdentity::generate(device);
                let trust = Trust::new(device);
                let this = Self { identity, trust };
                this.save(paths)?;
                Ok(this)
            }
            Err(source) => Err(IdentityError::Read {
                path: path.display().to_string(),
                source,
            }),
        }
    }

    /// Write atomically. A half-written identity file would cost the user
    /// every pairing they have.
    pub fn save(&self, paths: &Paths) -> Result<(), IdentityError> {
        let path = Self::file(paths);
        let stored = StoredRef {
            identity: &self.identity,
            trust: &self.trust,
        };
        let text = serde_json::to_string_pretty(&stored)?;
        write_atomically(&path, text.as_bytes())
    }
}

fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), IdentityError> {
    let err = |source: std::io::Error| IdentityError::Write {
        path: path.display().to_string(),
        source,
    };

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(err)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes).map_err(err)?;
    std::fs::rename(&tmp, path).map_err(err)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_run_generates_a_key_and_keeps_it() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(dir.path());
        let device = DeviceId::generate();

        let first = Identity::load_or_create(&paths, device).unwrap();
        let second = Identity::load_or_create(&paths, device).unwrap();

        assert_eq!(
            first.identity.public_key(),
            second.identity.public_key(),
            "regenerating the key would un-pair every device"
        );
        assert_eq!(second.trust.epoch(), 0);
    }

    #[test]
    fn paired_devices_survive_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(dir.path());
        let device = DeviceId::generate();
        let peer = clipse_crypto::PairedDevice {
            device_id: DeviceId::generate(),
            static_public: DeviceIdentity::generate(DeviceId::generate()).public_key(),
            label: "laptop".into(),
            platform: clipse_crypto::Platform::MacOs,
            addresses: vec![],
            paired_at_ms: 42,
        };

        let mut identity = Identity::load_or_create(&paths, device).unwrap();
        identity.trust.add_peer(peer.clone());
        identity.save(&paths).unwrap();

        let reloaded = Identity::load_or_create(&paths, device).unwrap();
        assert!(reloaded.trust.is_paired(&peer.device_id));
        assert_eq!(
            reloaded.trust.peer(&peer.device_id).unwrap().label,
            "laptop"
        );
    }

    #[test]
    fn a_mismatched_config_is_refused_rather_than_signing_with_the_wrong_key() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(dir.path());

        Identity::load_or_create(&paths, DeviceId::generate()).unwrap();
        let error = Identity::load_or_create(&paths, DeviceId::generate()).unwrap_err();

        assert!(matches!(error, IdentityError::Mismatch { .. }), "{error}");
    }

    #[test]
    fn a_corrupt_file_fails_loudly_rather_than_silently_regenerating() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(dir.path());
        paths.create_all().unwrap();
        std::fs::write(Identity::file(&paths), "{ not json").unwrap();

        let error = Identity::load_or_create(&paths, DeviceId::generate()).unwrap_err();
        assert!(matches!(error, IdentityError::Parse { .. }), "{error}");
    }
}
