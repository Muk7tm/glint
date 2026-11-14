// SPDX-License-Identifier: MIT
//! A throwaway X server for tests: Xvfb on a display it picks itself, stopped when dropped.
//!
//! Everything that injects input runs in one of these (or in Xephyr) until Phase 2's exit, never
//! on the real desktop (CLAUDE.md). Xvfb chooses a free display number and reports it once it
//! accepts connections (`-displayfd`), so tests running in parallel can't collide.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use crate::env::LocalDisplay;

#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    #[error("Xvfb is not installed")]
    NotInstalled,
    #[error("couldn't start Xvfb: {0}")]
    Start(String),
}

/// A running Xvfb.
pub struct Xvfb {
    child: Child,
    display: LocalDisplay,
}

impl Xvfb {
    /// Starts Xvfb with one `width × height` screen at 24-bit depth.
    pub fn start(width: u16, height: u16) -> Result<Self, SandboxError> {
        let mut child = Command::new("Xvfb")
            .args([
                "-displayfd",
                "1",
                "-screen",
                "0",
                &format!("{width}x{height}x24"),
                "-nolisten",
                "tcp",
            ])
            .args(["-noreset", "-br"]) // -br: a black root window instead of the default weave
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => SandboxError::NotInstalled,
                _ => SandboxError::Start(e.to_string()),
            })?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| SandboxError::Start("no stdout".into()))?;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            let _ = BufReader::new(stdout).read_line(&mut line);
            let _ = tx.send(line);
        });
        let number = match rx.recv_timeout(Duration::from_secs(15)) {
            Ok(line) => line.trim().parse::<u16>().ok(),
            Err(_) => None,
        };
        let Some(number) = number else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(SandboxError::Start("Xvfb didn't report a display".into()));
        };
        let real = LocalDisplay::from_env().ok().map(|d| d.number);
        if real == Some(number) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(SandboxError::Start(format!(":{number} is the real display")));
        }
        Ok(Self {
            child,
            display: LocalDisplay { number, screen: 0 },
        })
    }

    pub fn display(&self) -> LocalDisplay {
        self.display
    }

    /// The server's process id, for tests that freeze it.
    pub fn pid(&self) -> u32 {
        self.child.id()
    }
}

impl Drop for Xvfb {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
