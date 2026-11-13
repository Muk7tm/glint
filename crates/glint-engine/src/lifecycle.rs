// SPDX-License-Identifier: GPL-3.0-or-later
//! Startup and the stop paths that can't go through the hot loop (PLAN §4.3, §6): the panic hook
//! and the signals.
//!
//! Connection D is opened at startup and stays idle; its only job is the emergency release. A
//! release of something that isn't held does nothing, so the emergency path releases every button
//! Glint can press and every key it might hold, without knowing what is held.

use std::sync::Arc;

use arc_swap::ArcSwap;
use glint_x11::inject::release_everything_on;
use signal_hook::consts::signal::{SIGHUP, SIGINT, SIGTERM};
use x11rb::errors::ConnectionError;
use x11rb::rust_connection::RustConnection;

/// Connection D and what to release on it.
pub struct Emergency {
    conn: RustConnection,
    root: u32,
    /// Every modifier keycode and every key a profile uses; the controller keeps it current.
    keycodes: ArcSwap<Vec<u8>>,
}

impl Emergency {
    pub fn new(conn: RustConnection, root: u32, keycodes: Vec<u8>) -> Self {
        Self {
            conn,
            root,
            keycodes: ArcSwap::from_pointee(keycodes),
        }
    }

    pub fn set_keycodes(&self, keycodes: Vec<u8>) {
        self.keycodes.store(Arc::new(keycodes));
    }

    /// Releases every button Glint can press and every key it might hold.
    pub fn release_everything(&self) -> Result<(), ConnectionError> {
        release_everything_on(&self.conn, self.root, &self.keycodes.load())
    }
}

/// Releases input on connection D before the usual panic report. Release builds use
/// `panic = "abort"`, so the process ends right after; nothing else runs first (§4.3).
pub fn install_panic_hook(emergency: Arc<Emergency>) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = emergency.release_everything();
        previous(info);
    }));
}

/// SIGTERM, SIGINT and SIGHUP mean a graceful stop: `on_signal` runs on a thread of its own for
/// each one (it should release input, clean up and exit 0, §4.3).
pub fn watch_signals(on_signal: impl Fn(i32) + Send + 'static) -> std::io::Result<()> {
    let mut signals = signal_hook::iterator::Signals::new([SIGTERM, SIGINT, SIGHUP])?;
    std::thread::Builder::new()
        .name("glintd-signals".into())
        .spawn(move || {
            for signal in signals.forever() {
                on_signal(signal);
            }
        })?;
    Ok(())
}

/// `glintd`'s command line.
#[derive(clap::Parser, Debug, Default)]
#[command(
    name = "glintd",
    version,
    about = "The Glint engine: watches the screen and clicks (GPL-3.0-or-later)"
)]
pub struct Options {
    /// Start watching as soon as the engine is ready. Without it, the engine waits for Start,
    /// because watching never starts on its own.
    #[arg(long)]
    pub start_on_launch: bool,
    /// Exit when the process that started this engine exits (for `glint-lab`).
    #[arg(long, hide = true)]
    pub exit_with_parent: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum StartError {
    #[error(transparent)]
    Display(#[from] glint_x11::env::DisplayError),
    #[error("another engine is already running for display {0}")]
    AlreadyRunning(String),
    #[error("connecting to the X server: {0}")]
    Connect(#[from] x11rb::errors::ConnectError),
    #[error(transparent)]
    Wire(#[from] glint_x11::wire::WireError),
    #[error(transparent)]
    Caps(#[from] glint_x11::caps::CapsError),
    #[error(transparent)]
    Keymap(#[from] glint_x11::keymap::KeymapError),
    #[error(transparent)]
    Monitors(#[from] glint_x11::monitors::MonitorError),
    #[error(transparent)]
    Format(#[from] glint_x11::format::FormatError),
    #[error(transparent)]
    Compile(#[from] crate::controller::CompileError),
    #[error("the X server has no screen {0}")]
    NoScreen(u16),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("the socket directory {0} belongs to someone else")]
    ForeignSocketDir(String),
    #[error(transparent)]
    Lock(#[from] crate::lock::LockError),
    #[error(transparent)]
    Hotkeys(#[from] glint_x11::hotkeys::HotkeyError),
}

impl StartError {
    /// `glintd`'s exit code: 3 means "already running" (§4.3), 2 a missing or remote display.
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::AlreadyRunning(_) | Self::Lock(crate::lock::LockError::Held(_)) => 3,
            Self::Display(_) => 2,
            _ => 1,
        }
    }
}

/// The engine from start to exit (§4.3).
pub fn run(options: Options) -> Result<(), StartError> {
    use std::collections::BTreeMap;
    use std::sync::Mutex;
    use std::sync::mpsc;
    use std::time::Duration;

    use glint_core::{Lut, Profile, Settings};
    use glint_x11::capture::getimage::GetImageCapture;
    use glint_x11::conn::{self, ScreenInfo};
    use glint_x11::env::LocalDisplay;
    use glint_x11::format::PixelFormat;
    use glint_x11::inject::{Action, Injector, Input};
    use glint_x11::keymap::Keymap;
    use glint_x11::wire::RawConn;
    use glint_x11::{caps, monitors};

    use crate::controller::{
        Command, Controller, compile, release_keycodes, spawn_listener, spawn_x_events, watched_spot,
    };
    use crate::hot::HotHandle;
    use crate::lock::InstanceLock;
    use crate::store::{self, Store};
    use crate::telemetry::{self, Stats};
    use arc_swap::ArcSwap;
    use glint_x11::hotkeys::{self, Matcher, ModifierKeys};

    if options.exit_with_parent {
        // SIGTERM when the parent dies: the same graceful stop as any other SIGTERM.
        rustix::process::set_parent_process_death_signal(Some(rustix::process::Signal::TERM))
            .map_err(std::io::Error::from)?;
    }
    // 1. The display; remote displays are refused.
    let display = LocalDisplay::from_env()?;
    // 2. One engine per display: the lock is held until the process ends (C6, §4.4).
    let socket = prepare_socket(display)?;
    let _lock = InstanceLock::acquire(&glint_ipc::socket_dir(), display)?;

    // 3. X connections A–D and the capability probe; missing XTEST is fatal.
    let c = Arc::new(conn::open(display)?);
    let caps = caps::probe(&*c)?;
    let (a, setup) = RawConn::connect(display)?;
    let (b, _) = RawConn::connect(display)?;
    let screen = ScreenInfo::from_setup(&setup, display.screen).ok_or(StartError::NoScreen(display.screen))?;
    let keymap = Keymap::fetch(&*c)?;
    let monitors = monitors::list(&*c, screen.root, (screen.width, screen.height), caps.randr)?;
    let compositor = compositor_running(&c, display.screen);

    // 5. Settings and profiles from disk, migrating older schemas (§11).
    let mut store = Store::new(store::config_dir());
    let loaded = store.load();
    for problem in &loaded.problems {
        eprintln!("glintd: {problem}");
    }
    let settings: Settings = loaded.settings;
    let active = settings.active_profile.clone();
    let mut profiles: BTreeMap<String, Profile> = loaded.profiles;

    // 4. The defensive release: anything a killed engine left pressed (C10).
    let keycodes = release_keycodes(profiles.values(), &keymap);
    let mut injector = Injector::new(
        b,
        screen.root,
        caps.xtest_opcode,
        Action::new(Input::Button(1), Duration::ZERO),
    )?;
    injector.release_everything(&keycodes)?;
    let emergency = Arc::new(Emergency::new(conn::open(display)?, screen.root, keycodes));
    install_panic_hook(Arc::clone(&emergency));

    // The active profile, compiled on a pool smaller than the machine.
    let threads = std::thread::available_parallelism().map_or(1, |n| (n.get() / 2).max(1));
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .thread_name(|i| format!("glintd-lut-{i}"))
        .build()
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    let profile = profiles
        .get(&active)
        .cloned()
        .unwrap_or_else(|| Profile::new(active.clone()));
    let mut lut = Arc::new(pool.install(|| Lut::build(&profile.rules())));
    let compiled = match compile(&profile, &keymap, Arc::clone(&lut), watched_spot(&monitors, &profile)) {
        Ok(c) => c,
        Err(e) => {
            // A profile that can't run here (a modifier this keyboard lacks): start without it.
            eprintln!("glintd: profile {active:?}: {e}; starting without it");
            let blank = Profile::new(active.clone());
            profiles.insert(active.clone(), blank.clone());
            lut = Arc::new(pool.install(|| Lut::build(&blank.rules())));
            compile(&blank, &keymap, Arc::clone(&lut), (0, 0))?
        }
    };

    // 6. The socket, then the hot, telemetry, signal and IPC threads.
    let listener = std::os::unix::net::UnixListener::bind(&socket)?;
    let fmt = PixelFormat::root(&setup, display.screen.into())?;
    let capture = GetImageCapture::new(a, screen.root, fmt);
    let (hot, events, preview) = HotHandle::spawn(Box::new(capture), injector, compiled)?;
    let stats = Arc::new(Mutex::new(Stats::default()));
    let (commands, receiver) = mpsc::channel();
    {
        let (shared, stats, commands) = (Arc::clone(hot.shared()), Arc::clone(&stats), commands.clone());
        std::thread::Builder::new()
            .name("glintd-telemetry".into())
            .spawn(move || {
                telemetry::run(events, shared, stats, |n| {
                    let _ = commands.send(Command::Notice(n));
                });
            })?;
    }
    {
        let commands = commands.clone();
        watch_signals(move |s| {
            let _ = commands.send(Command::Signal(s));
        })?;
    }
    spawn_listener(listener, commands.clone())?;

    // 7. Hotkeys: XI2 raw key events on the root window, ignoring XTEST input (§9).
    let (bindings, problems) = hotkeys::bindings(&settings.hotkeys, &keymap);
    for p in problems {
        eprintln!("glintd: hotkey {p}");
    }
    let bindings = Arc::new(ArcSwap::from_pointee(bindings));
    let root_format = PixelFormat::root(&setup, display.screen.into())?;
    if caps.xinput.is_some() {
        hotkeys::select_raw_keys(&*c, screen.root)?;
        let matcher = Matcher::new(hotkeys::xtest_devices(&*c)?, ModifierKeys::new(&keymap));
        spawn_x_events(
            Arc::clone(&c),
            matcher,
            Arc::clone(&bindings),
            Arc::clone(hot.shared()),
            Arc::clone(&emergency),
            commands.clone(),
        )?;
    } else {
        eprintln!("glintd: the X server has no XInput 2; hotkeys are off");
    }
    let mut controller = Controller::new(
        display,
        caps,
        compositor,
        keymap,
        monitors,
        hot,
        preview,
        stats,
        emergency,
        settings,
        profiles,
        active,
        lut,
        receiver,
        pool,
        store,
        Arc::clone(&c),
        screen.root,
        root_format,
        bindings,
    );
    // 8. Ready and stopped; watching starts only when asked.
    if options.start_on_launch
        && let glint_ipc::Event::Error { message, .. } = controller.start()
    {
        eprintln!("glintd: --start-on-launch: {message}");
    }
    controller.run();
    // Dropping the controller stops the hot thread, which releases input before it exits.
    drop(controller);
    let _ = std::fs::remove_file(&socket);
    Ok(())
}

/// Creates the socket directory (mode 0700, ours only) and clears a stale socket (§10).
fn prepare_socket(display: glint_x11::env::LocalDisplay) -> Result<std::path::PathBuf, StartError> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    let dir = glint_ipc::socket_dir();
    std::fs::create_dir_all(&dir)?;
    if std::fs::metadata(&dir)?.uid() != rustix::process::getuid().as_raw() {
        return Err(StartError::ForeignSocketDir(dir.display().to_string()));
    }
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    let path = glint_ipc::socket_path(display.number);
    if path.exists() {
        if std::os::unix::net::UnixStream::connect(&path).is_ok() {
            return Err(StartError::AlreadyRunning(display.canonical()));
        }
        std::fs::remove_file(&path)?;
    }
    Ok(path)
}

/// A compositor owns the selection `_NET_WM_CM_S<screen>` (§8).
fn compositor_running(conn: &RustConnection, screen: u16) -> bool {
    use x11rb::protocol::xproto::ConnectionExt as _;
    let name = format!("_NET_WM_CM_S{screen}");
    let Ok(Ok(atom)) = conn.intern_atom(false, name.as_bytes()).map(|c| c.reply()) else {
        return false;
    };
    conn.get_selection_owner(atom.atom)
        .ok()
        .and_then(|c| c.reply().ok())
        .is_some_and(|r| r.owner != x11rb::NONE)
}
