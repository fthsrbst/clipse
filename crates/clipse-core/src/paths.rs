//! Where Clipse keeps its data on each platform.
//!
//! Everything is resolved through one struct so tests (and the `--data-dir`
//! flag used by the two-daemon end-to-end test) can point at a temp directory
//! without any other crate knowing about it.

use std::path::{Path, PathBuf};

use directories::ProjectDirs;

use crate::error::{Error, Result};

#[derive(Clone, Debug)]
pub struct Paths {
    root: PathBuf,
}

impl Paths {
    /// Platform default: `%APPDATA%\Clipse` / `~/Library/Application Support/dev.clipse.Clipse`
    /// / `~/.local/share/clipse`.
    pub fn platform_default() -> Result<Self> {
        let dirs = ProjectDirs::from("dev", "clipse", "Clipse").ok_or(Error::NoDataDirectory)?;
        Ok(Self { root: dirs.data_dir().to_path_buf() })
    }

    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn database(&self) -> PathBuf {
        self.root.join("clipse.db")
    }

    pub fn blobs(&self) -> PathBuf {
        self.root.join("blobs")
    }

    pub fn config(&self) -> PathBuf {
        self.root.join("config.toml")
    }

    pub fn logs(&self) -> PathBuf {
        self.root.join("logs")
    }

    pub fn create_all(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.root)?;
        std::fs::create_dir_all(self.blobs())?;
        std::fs::create_dir_all(self.logs())
    }

    /// Address the UI uses to reach the daemon: a named pipe on Windows, a unix
    /// socket elsewhere. Derived from the data root so two daemons running
    /// against different roots (the e2e test) do not collide.
    pub fn ipc_endpoint(&self) -> String {
        #[cfg(windows)]
        {
            let tag = short_tag(&self.root);
            format!(r"\\.\pipe\clipse-{tag}")
        }
        #[cfg(not(windows))]
        {
            self.root.join("clipsed.sock").to_string_lossy().into_owned()
        }
    }
}

#[cfg(windows)]
fn short_tag(root: &Path) -> String {
    // Named pipes live in a single flat namespace, so the root path is hashed
    // rather than embedded.
    crate::hash::ContentHash::of(root.to_string_lossy().as_bytes()).to_hex()[..12].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_hang_off_the_root() {
        let p = Paths::with_root("/tmp/clipse-test");
        assert!(p.database().starts_with(p.root()));
        assert!(p.blobs().starts_with(p.root()));
        assert!(p.config().starts_with(p.root()));
    }

    #[test]
    fn distinct_roots_get_distinct_endpoints() {
        let a = Paths::with_root("/tmp/a").ipc_endpoint();
        let b = Paths::with_root("/tmp/b").ipc_endpoint();
        assert_ne!(a, b, "two daemons would fight over one endpoint");
    }
}
