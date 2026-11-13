// SPDX-License-Identifier: MIT
//! One engine per display (PLAN §4.4): two engines watching the same screen would double every
//! click, so this is a correctness feature. The engine holds a non-blocking exclusive `flock` on
//! `<socket dir>/glintd-<N>.lock` for its whole life. `N` is the canonical display number, so
//! `:0`, `:0.0` and `unix:0` all take the same lock (C6). The kernel drops the lock when the
//! process ends, however it ends.

use std::fs::File;
use std::path::{Path, PathBuf};

use glint_x11::env::LocalDisplay;
use rustix::fs::FlockOperation;

#[derive(Debug, thiserror::Error)]
pub enum LockError {
    #[error("another engine is already running for display {0}")]
    Held(String),
    #[error("the lock file {0}: {1}")]
    Io(String, std::io::Error),
}

/// Held for as long as this value lives.
#[derive(Debug)]
pub struct InstanceLock {
    _file: File,
    path: PathBuf,
}

impl InstanceLock {
    pub fn acquire(dir: &Path, display: LocalDisplay) -> Result<Self, LockError> {
        let path = dir.join(format!("glintd-{}.lock", display.number));
        let io = |e| LockError::Io(path.display().to_string(), e);
        let file = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(io)?;
        match rustix::fs::flock(&file, FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => Ok(Self { _file: file, path }),
            Err(rustix::io::Errno::WOULDBLOCK) => Err(LockError::Held(display.canonical())),
            Err(e) => Err(io(e.into())),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_holder_per_display_however_it_is_spelled() {
        let dir = std::env::temp_dir().join(format!("glint-lock-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let first = InstanceLock::acquire(&dir, LocalDisplay::parse(":7").unwrap()).unwrap();
        for spelling in [":7", ":7.0", "unix:7", "unix:7.1"] {
            let again = InstanceLock::acquire(&dir, LocalDisplay::parse(spelling).unwrap());
            assert!(
                matches!(again, Err(LockError::Held(ref d)) if d == ":7"),
                "{spelling}: {again:?}"
            );
        }
        // Another display is independent, and the lock frees when its holder goes away.
        let other = InstanceLock::acquire(&dir, LocalDisplay::parse(":8").unwrap()).unwrap();
        drop(first);
        let after = InstanceLock::acquire(&dir, LocalDisplay::parse(":7").unwrap()).unwrap();
        assert!(after.path().ends_with("glintd-7.lock"));
        drop((other, after));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
