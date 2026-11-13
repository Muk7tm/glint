// SPDX-License-Identifier: MIT
//! Where the engine under test runs: a throwaway Xvfb ([`Sandbox`]) or the real desktop
//! ([`RealDesktop`]).

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use glint_core::pattern::Rect;
use glint_ipc::{Client, ClientKind, Request};
use glint_x11::env::LocalDisplay;
use rustix::process::{Pid, Signal, kill_process};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::ConnectionExt as _;

use crate::{LabError, WAIT, engine_err, x};

/// An engine the lab can drive.
pub trait Host {
    fn display(&self) -> LocalDisplay;
    fn socket(&self) -> &Path;
    /// The monitor the lab profile watches.
    fn monitor(&self) -> Result<Rect, LabError>;
    /// A control connection that has said hello.
    fn client(&self) -> Result<Client, LabError>;
    fn is_real(&self) -> bool;
    fn engine_pid(&self) -> Option<Pid>;

    fn signal(&self, signal: Signal) -> Result<(), LabError> {
        let pid = self
            .engine_pid()
            .ok_or_else(|| LabError::Setup("no engine is running".into()))?;
        kill_process(pid, signal).map_err(|e| LabError::Setup(format!("signal to the engine: {e}")))
    }

    /// The engine's resident memory, KiB.
    fn engine_rss_kib(&self) -> Option<u64> {
        let pid = self.engine_pid()?.as_raw_nonzero().get();
        let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
        status
            .lines()
            .find_map(|l| l.strip_prefix("VmRSS:"))
            .and_then(|v| v.trim().trim_end_matches("kB").trim().parse().ok())
    }

    /// The CPU time the engine has used so far, user and system, all threads.
    fn engine_cpu_time(&self) -> Option<Duration> {
        let pid = self.engine_pid()?.as_raw_nonzero().get();
        cpu_time(&std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
    }

    /// The CPU time of the engine's thread called `name` (`glintd-hot` checks the screen).
    fn engine_thread_cpu_time(&self, name: &str) -> Option<Duration> {
        let pid = self.engine_pid()?.as_raw_nonzero().get();
        std::fs::read_dir(format!("/proc/{pid}/task"))
            .ok()?
            .flatten()
            .find(|t| std::fs::read_to_string(t.path().join("comm")).is_ok_and(|c| c.trim_end() == name))
            .and_then(|t| cpu_time(&std::fs::read_to_string(t.path().join("stat")).ok()?))
    }

    /// SIGKILL, without any chance to release input.
    fn kill_engine(&mut self) -> Result<(), LabError>;
    /// Starts the engine again with the same configuration.
    fn restart_engine(&mut self) -> Result<(), LabError>;
    /// Starts a second engine on the same display and returns its exit code, or `None` if it
    /// kept running (it is then killed).
    fn second_engine(&self) -> Result<Option<i32>, LabError>;
}

/// Waits until `child` accepts connections on `socket`. A stale socket file from a killed
/// engine refuses them, so this connects rather than looking for the file.
fn wait_until_listening(child: &mut Child, socket: &Path, glintd: &Path) -> Result<(), LabError> {
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(status) = child.try_wait().map_err(x)? {
            return Err(LabError::Engine(if status.code() == Some(3) {
                "an engine is already running on this display; quit it first (glint-ctl quit)".into()
            } else {
                format!("{} exited: {status}", glintd.display())
            }));
        }
        if Client::connect(socket).is_ok() {
            return Ok(());
        }
        if Instant::now() > deadline {
            return Err(LabError::Engine("glintd didn't start".into()));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn hello(socket: &Path, kind: ClientKind) -> Result<Client, LabError> {
    let mut c = Client::connect(socket).map_err(engine_err)?;
    c.hello(kind, WAIT).map_err(engine_err)?;
    Ok(c)
}

/// Starts picom on `display` when `GLINT_LAB_COMPOSITOR=picom` asks for the composited path
/// (§16's optional CI job), and waits until it owns the compositing selection.
fn start_compositor(display: LocalDisplay) -> Result<Option<Child>, LabError> {
    let Some(which) = std::env::var_os("GLINT_LAB_COMPOSITOR").filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    if which != "picom" {
        return Err(LabError::Setup(format!(
            "GLINT_LAB_COMPOSITOR={which:?}: only picom is supported"
        )));
    }
    let mut child = Command::new("picom")
        .args(["--backend", "xrender", "--config", "/dev/null"])
        .env("DISPLAY", display.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| LabError::Setup(format!("picom: {e}")))?;
    let conn = glint_x11::conn::open(display).map_err(x)?;
    let name = format!("_NET_WM_CM_S{}", display.screen);
    let atom = conn
        .intern_atom(false, name.as_bytes())
        .map_err(x)?
        .reply()
        .map_err(x)?
        .atom;
    let deadline = Instant::now() + WAIT;
    loop {
        if conn.get_selection_owner(atom).map_err(x)?.reply().map_err(x)?.owner != x11rb::NONE {
            return Ok(Some(child));
        }
        if let Some(status) = child.try_wait().map_err(x)? {
            return Err(LabError::Setup(format!("picom exited: {status}")));
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(LabError::Setup("picom didn't start compositing".into()));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A throwaway Xvfb with a real `glintd` on it, its runtime and config directories private to it.
/// Nothing here touches the real desktop.
pub struct Sandbox {
    xvfb: glint_x11::sandbox::Xvfb,
    compositor: Option<Child>,
    engine: Option<Child>,
    glintd: PathBuf,
    scratch: PathBuf,
    socket: PathBuf,
}

impl Sandbox {
    /// `Ok(None)` when Xvfb isn't installed.
    pub fn start(glintd: &Path, width: u16, height: u16) -> Result<Option<Self>, LabError> {
        use glint_x11::sandbox::{SandboxError, Xvfb};
        let xvfb = match Xvfb::start(width, height) {
            Ok(x) => x,
            Err(SandboxError::NotInstalled) => return Ok(None),
            Err(e) => return Err(x(e)),
        };
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let scratch = std::env::temp_dir().join(format!("glint-lab-{}-{n}", std::process::id()));
        std::fs::create_dir_all(scratch.join("run")).map_err(x)?;
        // Sandboxed engines run at normal priority: many of them asking rtkit would hit its
        // per-user limits, and real-time hot threads could starve the sandboxes' X servers
        // (ADR 0017). The real desktop keeps the user's choice.
        let config = scratch.join("config/glint");
        std::fs::create_dir_all(&config).map_err(x)?;
        std::fs::write(
            config.join("settings.toml"),
            "schema_version = 1\n[engine]\nrealtime = \"off\"\n",
        )
        .map_err(x)?;
        let socket = scratch
            .join("run/glint")
            .join(format!("glintd-{}.sock", xvfb.display().number));
        let mut sandbox = Self {
            compositor: start_compositor(xvfb.display())?,
            xvfb,
            engine: None,
            glintd: glintd.to_owned(),
            scratch,
            socket,
        };
        sandbox.restart_engine()?;
        Ok(Some(sandbox))
    }

    fn command(&self) -> Command {
        let mut c = Command::new(&self.glintd);
        c.env("DISPLAY", self.xvfb.display().to_string())
            .env("XDG_RUNTIME_DIR", self.scratch.join("run"))
            .env("XDG_CONFIG_HOME", self.scratch.join("config"))
            .env("XDG_STATE_HOME", self.scratch.join("state"))
            .stdin(Stdio::null());
        c
    }

    /// The whole screen, which is the sandbox's only monitor.
    pub fn screen(&self) -> Result<Rect, LabError> {
        let conn = glint_x11::conn::open(self.display()).map_err(x)?;
        let s = &conn.setup().roots[0];
        Ok(Rect::new(0, 0, s.width_in_pixels, s.height_in_pixels))
    }

    /// Whether picom runs on this sandbox.
    pub fn composited(&self) -> bool {
        self.compositor.is_some()
    }
}

impl Host for Sandbox {
    fn display(&self) -> LocalDisplay {
        self.xvfb.display()
    }

    fn socket(&self) -> &Path {
        &self.socket
    }

    fn monitor(&self) -> Result<Rect, LabError> {
        self.screen()
    }

    fn client(&self) -> Result<Client, LabError> {
        hello(&self.socket, ClientKind::Lab)
    }

    fn is_real(&self) -> bool {
        false
    }

    fn engine_pid(&self) -> Option<Pid> {
        self.engine.as_ref().map(Pid::from_child)
    }

    fn kill_engine(&mut self) -> Result<(), LabError> {
        if let Some(mut e) = self.engine.take() {
            e.kill().map_err(x)?;
            e.wait().map_err(x)?;
        }
        Ok(())
    }

    fn restart_engine(&mut self) -> Result<(), LabError> {
        let mut child = self
            .command()
            .spawn()
            .map_err(|e| LabError::Engine(format!("{}: {e}", self.glintd.display())))?;
        let started = wait_until_listening(&mut child, &self.socket, &self.glintd);
        if started.is_err() {
            let _ = child.kill();
            let _ = child.wait();
        }
        started?;
        self.engine = Some(child);
        Ok(())
    }

    fn second_engine(&self) -> Result<Option<i32>, LabError> {
        let mut child = self
            .command()
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| LabError::Engine(format!("{}: {e}", self.glintd.display())))?;
        let deadline = Instant::now() + WAIT;
        while Instant::now() < deadline {
            if let Some(status) = child.try_wait().map_err(x)? {
                return Ok(status.code());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = child.kill();
        let _ = child.wait();
        Ok(None)
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        for child in [self.engine.as_mut(), self.compositor.as_mut()].into_iter().flatten() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.scratch);
    }
}

/// An engine on the real desktop for `glint-lab --real-desktop`: a fresh `glintd` on `$DISPLAY`
/// with a throwaway config directory (your profiles and settings stay untouched) but the real
/// runtime directory, so the single-instance lock refuses it if an engine already runs there. It
/// exits with the lab, and the lab connects as a UI, so if the lab dies the engine stops watching
/// at once.
pub struct RealDesktop {
    engine: Child,
    scratch: PathBuf,
    socket: PathBuf,
    display: LocalDisplay,
}

impl RealDesktop {
    pub fn start(glintd: &Path) -> Result<Self, LabError> {
        let display = LocalDisplay::from_env().map_err(x)?;
        let scratch = std::env::temp_dir().join(format!("glint-lab-real-{}", std::process::id()));
        std::fs::create_dir_all(&scratch).map_err(x)?;
        let mut engine = Command::new(glintd)
            .arg("--exit-with-parent")
            .env("XDG_CONFIG_HOME", scratch.join("config"))
            .env("XDG_STATE_HOME", scratch.join("state"))
            .stdin(Stdio::null())
            .spawn()
            .map_err(|e| LabError::Engine(format!("{}: {e}", glintd.display())))?;
        let socket = glint_ipc::socket_path(display.number);
        if let Err(e) = wait_until_listening(&mut engine, &socket, glintd) {
            let _ = engine.kill();
            let _ = engine.wait();
            let _ = std::fs::remove_dir_all(&scratch);
            return Err(e);
        }
        Ok(Self {
            engine,
            scratch,
            socket,
            display,
        })
    }
}

const SANDBOX_ONLY: &str = "only in the sandbox";

impl Host for RealDesktop {
    fn display(&self) -> LocalDisplay {
        self.display
    }

    fn socket(&self) -> &Path {
        &self.socket
    }

    /// The monitor the lab profile watches (`"primary"`, or the first when none is).
    fn monitor(&self) -> Result<Rect, LabError> {
        let conn = glint_x11::conn::open(self.display).map_err(x)?;
        let root = conn.setup().roots[usize::from(self.display.screen)].root;
        let size = (
            conn.setup().roots[0].width_in_pixels,
            conn.setup().roots[0].height_in_pixels,
        );
        let caps = glint_x11::caps::probe(&conn).map_err(x)?;
        let list = glint_x11::monitors::list(&conn, root, size, caps.randr).map_err(x)?;
        glint_x11::monitors::pick(&list, "primary")
            .map(|m| m.rect)
            .ok_or_else(|| LabError::X("no monitor".into()))
    }

    /// A client that identifies as a UI, so its disappearance stops watching.
    fn client(&self) -> Result<Client, LabError> {
        hello(&self.socket, ClientKind::Ui)
    }

    fn is_real(&self) -> bool {
        true
    }

    fn engine_pid(&self) -> Option<Pid> {
        Some(Pid::from_child(&self.engine))
    }

    fn kill_engine(&mut self) -> Result<(), LabError> {
        Err(LabError::Setup(SANDBOX_ONLY.into()))
    }

    fn restart_engine(&mut self) -> Result<(), LabError> {
        Err(LabError::Setup(SANDBOX_ONLY.into()))
    }

    fn second_engine(&self) -> Result<Option<i32>, LabError> {
        Err(LabError::Setup(SANDBOX_ONLY.into()))
    }
}

impl Drop for RealDesktop {
    fn drop(&mut self) {
        // A frozen engine (the stall scenario) must run again to quit.
        let _ = self.signal(Signal::CONT);
        if let Ok(mut c) = Client::connect(&self.socket) {
            let _ = c.call(Request::Quit, WAIT);
        }
        let deadline = Instant::now() + WAIT;
        while self.engine.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.engine.kill();
        let _ = self.engine.wait();
        let _ = std::fs::remove_dir_all(&self.scratch);
    }
}

/// The CPU time in a `/proc/<pid>/stat` line: `utime` plus `stime`, fields 14 and 15, in clock
/// ticks. The command name before them is in parentheses and may hold spaces or parentheses, so
/// fields are counted from the last `)`.
fn cpu_time(stat: &str) -> Option<Duration> {
    let fields: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
    // After the name, field 3 (the state) comes first: utime is field 14, at index 11.
    let ticks = fields.get(11)?.parse::<u64>().ok()? + fields.get(12)?.parse::<u64>().ok()?;
    let per_second = rustix::param::clock_ticks_per_second();
    (per_second > 0).then(|| Duration::from_secs_f64(ticks as f64 / per_second as f64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_time_adds_user_and_system_time() {
        let hz = rustix::param::clock_ticks_per_second() as f64;
        // A name with a space and a parenthesis; utime 150 and stime 50 ticks.
        let stat = "4242 (glint d)) S 1 4242 4242 0 -1 4194304 136 0 1 0 150 50 0 0 20 0 3 0 9368063";
        assert_eq!(cpu_time(stat), Some(Duration::from_secs_f64(200.0 / hz)));
        assert_eq!(cpu_time("4242 (glintd"), None);
        assert_eq!(cpu_time("4242 (glintd) S 1"), None);
        // This test's own process has used some CPU time by now.
        let own = std::fs::read_to_string("/proc/self/stat").unwrap();
        assert!(cpu_time(&own).is_some());
    }
}
