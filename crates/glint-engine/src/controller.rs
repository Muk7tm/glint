// SPDX-License-Identifier: GPL-3.0-or-later
//! The controller (PLAN §4.2): IPC, settings and profiles, lookup-table builds, monitors and,
//! later, windows and hotkeys on X connection C. It is the only thread that changes what the hot
//! thread does, through [`HotHandle`], and it never waits on a client: each client has a bounded
//! outbound queue, and a client that stops reading loses telemetry first, then its connection.

use std::collections::{BTreeMap, HashMap};
use std::io::{BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use glint_core::color::oklab::Oklch;
use glint_core::pattern::{PatternError, Rect};
use glint_core::profile::{ActionKind, ClickAt, Modifier, Profile, TargetMode};
use glint_core::settings::HotkeyMode;
use glint_core::settings::{ClosePolicy, Settings};
use glint_core::validate::{self, Issue};
use glint_core::{Lut, Rgb};
use glint_ipc::{
    CapsInfo, ClientKind, Envelope, Event, MAX_CLASSIFY, PROTOCOL, Phase, Request, SessionInfo, StateSnapshot,
    StopCause, TargetInfo, TargetKind, TelemetrySnapshot, Trace, WindowState, WindowStatus, monotonic_ns,
};
use glint_x11::caps::Caps;
use glint_x11::env::LocalDisplay;
use glint_x11::format::PixelFormat;
use glint_x11::hotkeys::{self, Binding, HotkeyAction, Matcher, ModifierKeys};
use glint_x11::inject::{Action, Input, MAX_MODIFIERS};
use glint_x11::keymap::{Keymap, keysym};
use glint_x11::monitors::{self, Monitor};
use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::{ConnectionExt as _, ImageFormat};
use x11rb::rust_connection::RustConnection;

use crate::desktop::{DesktopEvent, Watch};
use crate::hot::{CompiledProfile, Control, HotHandle, HotShared, Preview, StopReason};
use crate::lifecycle::Emergency;
use crate::pick::{self, PickInput};
use crate::store::Store;
use crate::target::{Change, Tracker, WinEvent};
use crate::telemetry::{Notice, Stats, TraceNotice};

/// How often the controller sends telemetry and live updates and tidies up.
const TICK: Duration = Duration::from_millis(20);
/// Each client's outbound queue (§10).
const OUTBOX: usize = 256;
/// After an emergency stop, Start is ignored this long, so a bouncing key can't restart (§9).
const EMERGENCY_HOLDOFF: Duration = Duration::from_millis(500);
/// `fired` events are coalesced to at most 20 a second (§10).
const FIRED_EVERY: Duration = Duration::from_millis(50);

#[derive(Debug, thiserror::Error)]
pub enum CompileError {
    #[error(transparent)]
    Pattern(#[from] PatternError),
    #[error("the key action has no key")]
    NoKey,
    #[error("the keyboard has no {0:?} key")]
    NoModifier(Modifier),
}

/// The keysym a modifier name stands for.
fn modifier_keysym(m: Modifier) -> u32 {
    match m {
        Modifier::Shift => keysym::SHIFT_L,
        Modifier::Ctrl => keysym::CONTROL_L,
        Modifier::Alt => keysym::ALT_L,
        Modifier::Super => keysym::SUPER_L,
    }
}

/// Turns a validated profile into what the hot loop runs. The lookup table is built separately
/// (it takes a few hundred milliseconds); `watched_spot` is where *Click at: Watched spot* moves
/// the pointer, in root coordinates.
pub fn compile(
    profile: &Profile,
    keymap: &Keymap,
    lut: Arc<Lut>,
    watched_spot: (i16, i16),
) -> Result<CompiledProfile, CompileError> {
    let a = &profile.action;
    let input = match a.kind {
        ActionKind::Mouse => Input::Button(a.button.x_button()),
        ActionKind::Key => Input::Key(a.key.as_ref().ok_or(CompileError::NoKey)?.keycode),
    };
    let mut modifiers = [0u8; MAX_MODIFIERS];
    let n = a.modifiers.len().min(MAX_MODIFIERS);
    for (slot, &m) in modifiers.iter_mut().zip(&a.modifiers) {
        *slot = keymap.keycode(modifier_keysym(m)).ok_or(CompileError::NoModifier(m))?;
    }
    let warp = (a.click_at == ClickAt::WatchedSpot).then_some(watched_spot);
    let action = Action::new(input, Duration::from_millis(a.hold_ms.into()))
        .with_modifiers(&modifiers[..n])
        .with_warp(warp);
    Ok(CompiledProfile {
        revision: 0,
        lut,
        pattern: profile.pattern()?,
        trigger: profile.trigger_cfg(),
        action,
        period: profile.sampling.speed.period(),
    })
}

/// The watched spot in root coordinates (the monitor's anchor pixel plus the offset), where
/// *Click at: Watched spot* moves the pointer.
pub fn watched_spot(monitors: &[Monitor], profile: &Profile) -> (i16, i16) {
    match monitors::pick(monitors, &profile.target.monitor) {
        Some(m) => {
            let (x, y) = m.rect.anchor();
            (
                (x + profile.target.offset.x) as i16,
                (y + profile.target.offset.y) as i16,
            )
        }
        None => (0, 0),
    }
}

/// A trace entry with its times as `CLOCK_MONOTONIC` nanoseconds.
fn trace_entry(t: TraceNotice) -> Trace {
    match t {
        TraceNotice::Fired {
            request,
            capture,
            decided,
            sent,
        } => Trace::Fired {
            request_ns: monotonic_ns(request),
            capture_ns: monotonic_ns(capture),
            decided_ns: monotonic_ns(decided),
            sent_ns: monotonic_ns(sent),
        },
        TraceNotice::MaybeMissed { at } => Trace::MaybeMissed {
            at_ns: monotonic_ns(at),
        },
        TraceNotice::Blind { from, to } => Trace::Blind {
            from_ns: monotonic_ns(from),
            to_ns: monotonic_ns(to),
        },
        TraceNotice::Stopped { at, reason } => Trace::Stopped {
            at_ns: monotonic_ns(at),
            cause: match reason {
                StopReason::SafetyStop => StopCause::SafetyStop,
                StopReason::TargetGone => StopCause::TargetGone,
                StopReason::CaptureFailed => StopCause::CaptureFailed,
                StopReason::InputFailed => StopCause::InputFailed,
            },
        },
    }
}

/// Every keycode the given profiles' actions could hold, plus every modifier key, for the
/// emergency and defensive releases (C10).
pub fn release_keycodes<'a>(profiles: impl IntoIterator<Item = &'a Profile>, keymap: &Keymap) -> Vec<u8> {
    let mut out = keymap.modifier_keycodes();
    out.extend(
        profiles
            .into_iter()
            .filter_map(|p| p.action.key.as_ref())
            .map(|k| k.keycode),
    );
    out.sort_unstable();
    out.dedup();
    out
}

pub type ClientId = u64;

/// Why a profile was refused: the message and the field it belongs to.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Rejected {
    pub message: String,
    pub field: Option<String>,
}

impl From<Rejected> for Event {
    fn from(r: Rejected) -> Self {
        Event::error("invalid", r.message, r.field)
    }
}

/// What the controller thread acts on.
pub enum Command {
    Connected {
        client: ClientId,
        outbox: SyncSender<String>,
        writer: std::thread::JoinHandle<()>,
    },
    Request {
        client: ClientId,
        id: Option<u64>,
        request: Request,
    },
    Malformed {
        client: ClientId,
        message: String,
    },
    Disconnected {
        client: ClientId,
    },
    Notice(Notice),
    Signal(i32),
    /// A hotkey was pressed (`true`) or released.
    Hotkey {
        action: HotkeyAction,
        press: bool,
    },
    /// The keyboard mapping changed: hotkeys and modifiers need resolving again.
    KeymapChanged,
    /// The pointer or keyboard during a color or window pick (§10 `pick_color`, `pick_window`).
    Pick(PickInput),
    /// Something happened to a window the tracker watches, or to the window list (§8).
    Window(WinEvent),
    /// A compositor came or went, or the screen locked or unlocked (§8).
    Desktop(DesktopEvent),
}

/// The core events a pick's grab delivers.
fn pick_input(event: &x11rb::protocol::Event) -> Option<PickInput> {
    use x11rb::protocol::Event as E;
    match event {
        E::MotionNotify(e) => Some(PickInput::Moved(e.root_x, e.root_y)),
        E::ButtonPress(e) => Some(PickInput::Clicked(e.detail, e.root_x, e.root_y, e.child)),
        E::KeyPress(e) => Some(PickInput::Key(e.detail)),
        _ => None,
    }
}

/// What a pick is for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PickKind {
    Color,
    Window,
}

/// A color or window pick in progress.
struct Picking {
    client: ClientId,
    kind: PickKind,
    escape: Option<u8>,
    /// Where the pointer went since the last magnifier frame.
    pending: Option<(i16, i16)>,
    last_frame: Option<Instant>,
}

/// About 30 magnifier frames a second.
const PICK_FRAME: Duration = Duration::from_millis(33);
/// `XK_Escape`.
const ESCAPE: u32 = 0xff1b;

/// Reads connection C's events: hotkeys from XI2 raw key events (§9) and keymap changes.
///
/// The emergency stop doesn't wait for the controller, which may be busy building a lookup table:
/// this thread disarms the hot loop and releases everything on connection D itself, then tells the
/// controller.
pub fn spawn_x_events(
    conn: Arc<RustConnection>,
    desktop: Watch,
    mut matcher: Matcher,
    bindings: Arc<ArcSwap<Vec<Binding>>>,
    hot: Arc<HotShared>,
    emergency: Arc<Emergency>,
    commands: Sender<Command>,
) -> std::io::Result<()> {
    std::thread::Builder::new().name("glintd-x".into()).spawn(move || {
        while let Ok(event) = conn.wait_for_event() {
            if let Some(key) = hotkeys::raw_key(&event) {
                let Some((action, press)) = matcher.feed(key, &bindings.load()) else {
                    continue;
                };
                if action == HotkeyAction::EmergencyStop && press {
                    hot.control.update(|c| Control {
                        armed: false,
                        ..c.bumped()
                    });
                    let _ = emergency.release_everything();
                }
                if commands.send(Command::Hotkey { action, press }).is_err() {
                    break;
                }
            } else if let Some(input) = pick_input(&event) {
                if commands.send(Command::Pick(input)).is_err() {
                    break;
                }
            } else if let Some(w) = WinEvent::from_x(&event) {
                if commands.send(Command::Window(w)).is_err() {
                    break;
                }
            } else if let Some(d) = desktop.event(&event) {
                if commands.send(Command::Desktop(d)).is_err() {
                    break;
                }
            } else if matches!(event, x11rb::protocol::Event::MappingNotify(_)) {
                if let Ok(keymap) = Keymap::fetch(&*conn) {
                    matcher.set_modifier_keys(ModifierKeys::new(&keymap));
                }
                if commands.send(Command::KeymapChanged).is_err() {
                    break;
                }
            }
        }
    })?;
    Ok(())
}

/// Accepts connections from the same user only (§10) and turns each client's lines into
/// [`Command`]s. Each client gets a reader thread and a writer thread.
pub fn spawn_listener(listener: UnixListener, commands: Sender<Command>) -> std::io::Result<()> {
    let next = AtomicU64::new(1);
    std::thread::Builder::new().name("glintd-ipc".into()).spawn(move || {
        for stream in listener.incoming().flatten() {
            let same_user =
                rustix::net::sockopt::socket_peercred(&stream).is_ok_and(|c| c.uid == rustix::process::getuid());
            if same_user {
                accept(stream, next.fetch_add(1, Ordering::Relaxed), &commands);
            }
        }
    })?;
    Ok(())
}

fn accept(stream: UnixStream, client: ClientId, commands: &Sender<Command>) {
    let (Ok(mut writer), Ok(closer)) = (stream.try_clone(), stream.try_clone()) else {
        return;
    };
    let (outbox, lines) = mpsc::sync_channel::<String>(OUTBOX);
    let writer = std::thread::Builder::new()
        .name("glintd-client-w".into())
        .spawn(move || {
            for line in lines {
                if writer.write_all(line.as_bytes()).is_err() {
                    break;
                }
            }
            let _ = writer.shutdown(std::net::Shutdown::Both);
        });
    let Ok(writer) = writer else { return };
    if commands.send(Command::Connected { client, outbox, writer }).is_err() {
        return;
    }
    let commands = commands.clone();
    let _ = std::thread::Builder::new()
        .name("glintd-client-r".into())
        .spawn(move || {
            let mut reader = BufReader::new(stream);
            loop {
                match glint_ipc::read_line(&mut reader) {
                    Ok(Some(line)) if line.trim().is_empty() => {}
                    Ok(Some(line)) => {
                        let cmd = match serde_json::from_str::<Envelope<Request>>(&line) {
                            Ok(env) => Command::Request {
                                client,
                                id: env.id,
                                request: env.body,
                            },
                            Err(e) => Command::Malformed {
                                client,
                                message: e.to_string(),
                            },
                        };
                        if commands.send(cmd).is_err() {
                            break;
                        }
                    }
                    Ok(None) | Err(_) => break,
                }
            }
            let _ = closer.shutdown(std::net::Shutdown::Both);
            let _ = commands.send(Command::Disconnected { client });
        });
}

struct Client {
    outbox: SyncSender<String>,
    /// Joined at shutdown, so everything queued (the final `bye`) reaches the client.
    writer: Option<std::thread::JoinHandle<()>>,
    kind: Option<ClientKind>,
    detached: bool,
    telemetry_every: Option<Duration>,
    live_every: Option<Duration>,
    last_telemetry: Instant,
    last_live: Instant,
    /// Wants every trace entry as it happens (`glint-lab`).
    trace: bool,
}

/// Everything the controller owns.
pub struct Controller {
    pub display: LocalDisplay,
    pub caps: Caps,
    pub compositor: bool,
    pub keymap: Keymap,
    pub monitors: Vec<Monitor>,
    pub hot: HotHandle,
    pub preview: triple_buffer::Output<Preview>,
    pub stats: Arc<Mutex<Stats>>,
    pub emergency: Arc<Emergency>,
    pub settings: Settings,
    pub profiles: BTreeMap<String, Profile>,
    pub active: String,
    pub lut: Arc<Lut>,
    pub commands: Receiver<Command>,
    pub pool: rayon::ThreadPool,
    pub store: Store,
    /// Connection C, shared with the X-events thread.
    pub conn: Arc<RustConnection>,
    pub root: u32,
    pub root_format: PixelFormat,
    /// The hotkeys as the X-events thread matches them.
    pub bindings: Arc<ArcSwap<Vec<Binding>>>,
    revision: u64,
    started: bool,
    last_stop: Option<String>,
    start_blocked_until: Option<Instant>,
    clients: HashMap<ClientId, Client>,
    fired: Option<(u64, u32)>,
    last_fired: Instant,
    quit: bool,
    picking: Option<Picking>,
    /// The crosshair for picking, made on first use.
    cursor: Option<u32>,
    /// The picked game window (§8).
    tracker: Tracker,
    /// Where each UI's window is (`ui_geometry`), and whether one covers the watched spot.
    ui_rects: HashMap<ClientId, Rect>,
    covered: bool,
    /// The fastest monitor's refresh rate, for the check gap under a compositor.
    refresh_hz: Option<f64>,
}

impl Controller {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        display: LocalDisplay,
        caps: Caps,
        compositor: bool,
        keymap: Keymap,
        monitors: Vec<Monitor>,
        hot: HotHandle,
        preview: triple_buffer::Output<Preview>,
        stats: Arc<Mutex<Stats>>,
        emergency: Arc<Emergency>,
        settings: Settings,
        profiles: BTreeMap<String, Profile>,
        active: String,
        lut: Arc<Lut>,
        commands: Receiver<Command>,
        pool: rayon::ThreadPool,
        store: Store,
        conn: Arc<RustConnection>,
        root: u32,
        root_format: PixelFormat,
        bindings: Arc<ArcSwap<Vec<Binding>>>,
    ) -> Self {
        let tracker = Tracker::new(&conn, root);
        let refresh_hz = crate::desktop::max_refresh(&conn, root);
        Self {
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
            commands,
            pool,
            store,
            conn,
            root,
            root_format,
            bindings,
            revision: 1,
            started: false,
            last_stop: None,
            start_blocked_until: None,
            clients: HashMap::new(),
            fired: None,
            last_fired: Instant::now(),
            quit: false,
            picking: None,
            cursor: None,
            tracker,
            ui_rects: HashMap::new(),
            covered: false,
            refresh_hz,
        }
    }

    /// Runs until `quit` or a stop signal.
    pub fn run(&mut self) {
        self.acquire_window();
        let mut last_tick = Instant::now();
        while !self.quit {
            match self.commands.recv_timeout(TICK) {
                Ok(cmd) => self.handle(cmd),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            if last_tick.elapsed() >= TICK {
                last_tick = Instant::now();
                self.tick();
            }
        }
        self.broadcast(Event::Bye);
        // Close every queue, then give the writers up to a second to deliver what's left (the
        // final `bye`). A client that stopped reading can't hold the exit up longer than that.
        let writers: Vec<_> = self.clients.drain().filter_map(|(_, c)| c.writer).collect();
        let deadline = Instant::now() + Duration::from_secs(1);
        for w in writers {
            while !w.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
            if w.is_finished() {
                let _ = w.join();
            }
        }
        for problem in self.store.flush_all() {
            eprintln!("glintd: saving {problem}");
        }
    }

    /// Starts watching with the active profile, if it can (§4.3 "never on its own": callers are
    /// a Start request, a hotkey or `--start-on-launch`).
    pub fn start(&mut self) -> Event {
        if self.started {
            return Event::State(self.state());
        }
        if self.start_blocked_until.is_some_and(|t| Instant::now() < t) {
            return Event::error(
                "cannot_start",
                "Emergency stop: Start is ignored for half a second.",
                None,
            );
        }
        let profile = self.active_profile();
        if let Some(why) = profile.start_blocker() {
            return Event::error("cannot_start", why, None);
        }
        if let Some(why) = self.window_blocker(&profile) {
            return Event::error("cannot_start", why, Some("target.window".into()));
        }
        let origin = match self.origin(&profile) {
            Ok(o) => o,
            Err(issue) => return Event::error("cannot_start", issue.message, Some(issue.field)),
        };
        self.hot.start(origin, self.gate(&profile));
        self.started = true;
        self.last_stop = None;
        self.check_cover();
        Event::State(self.state())
    }

    /// Stops watching and releases input; `reason` is shown as the last stop.
    pub fn stop(&mut self, reason: Option<String>) {
        self.hot.stop();
        self.covered = false;
        if self.started {
            self.last_stop = reason;
        }
        self.started = false;
    }

    pub fn state(&self) -> StateSnapshot {
        let profile = self.active_profile();
        let control = self.hot.control();
        let monitor = monitors::pick(&self.monitors, &profile.target.monitor).map(|m| m.name.clone());
        StateSnapshot {
            started: self.started,
            phase: match (self.started, control.gate) {
                (false, _) => Phase::Stopped,
                (true, true) => Phase::Watching,
                (true, false) => Phase::Paused,
            },
            target: TargetInfo {
                kind: match profile.target.mode {
                    TargetMode::Screen => TargetKind::Screen,
                    TargetMode::Window => TargetKind::Window,
                },
                name: match profile.target.mode {
                    TargetMode::Screen => monitor.unwrap_or_default(),
                    TargetMode::Window => self.window_title(&profile),
                },
            },
            backend: "screen".into(),
            checks_per_sec: profile.sampling.speed.checks_per_second(),
            profile: profile.name.clone(),
            revision: self.revision,
            last_stop: self.last_stop.clone(),
            cannot_start: profile
                .start_blocker()
                .map(str::to_owned)
                .or_else(|| self.window_blocker(&profile)),
            window: self.window_status(&profile),
            covered: self.covered,
            compositor: self.compositor,
            check_gap_us: u32::try_from(self.check_gap(&profile).as_micros()).unwrap_or(u32::MAX),
        }
    }

    fn active_profile(&self) -> Profile {
        self.profiles
            .get(&self.active)
            .cloned()
            .unwrap_or_else(|| Profile::new(self.active.clone()))
    }

    /// Where the capture rectangle goes for `profile` on its monitor (§5.1).
    fn origin(&self, profile: &Profile) -> Result<(i16, i16), Issue> {
        let area = match profile.target.mode {
            TargetMode::Window => self
                .tracker
                .rect()
                .ok_or_else(|| Issue::new("target.window", "The game window isn't showing."))?,
            TargetMode::Screen => {
                monitors::pick(&self.monitors, &profile.target.monitor)
                    .ok_or_else(|| {
                        Issue::new(
                            "target.monitor",
                            format!("There's no monitor named {:?}.", profile.target.monitor),
                        )
                    })?
                    .rect
            }
        };
        let pattern = profile
            .pattern()
            .map_err(|e| Issue::new("sampling.pattern", e.to_string()))?;
        let rect = validate::placement(&pattern, area, (profile.target.offset.x, profile.target.offset.y)).map_err(
            |issue| match profile.target.mode {
                TargetMode::Window => Issue::new("target.window", "Window too small"),
                TargetMode::Screen => issue,
            },
        )?;
        Ok((rect.x as i16, rect.y as i16))
    }

    /// Validates, compiles and publishes a profile; the hot thread restarts watching with it,
    /// so a live edit can't click (§7 #8).
    pub fn apply_profile(&mut self, profile: Profile) -> Result<u64, Rejected> {
        let report = validate::profile(&profile);
        let keys = validate::action_against_hotkeys(&profile.action, &self.settings.hotkeys);
        if let Some(issue) = report.errors.into_iter().chain(keys.errors).next() {
            return Err(Rejected {
                message: issue.message,
                field: Some(issue.field),
            });
        }
        let rules = profile.rules();
        let lut = Arc::new(self.pool.install(|| Lut::build(&rules)));
        let compiled = compile(
            &profile,
            &self.keymap,
            Arc::clone(&lut),
            watched_spot(&self.monitors, &profile),
        )
        .map_err(|e| Rejected {
            message: e.to_string(),
            field: None,
        })?;
        let new_origin = self.origin(&profile);
        let mut compiled = compiled;
        compiled.trigger.max_check_gap = self.check_gap(&profile);
        self.hot.set_profile(compiled);
        self.lut = lut;
        self.revision += 1;
        let now = Instant::now();
        self.store.save_profile(&profile, now);
        if self.settings.active_profile != profile.name {
            self.settings.active_profile = profile.name.clone();
            self.store.save_settings(&self.settings, now);
        }
        self.active = profile.name.clone();
        let window_changed = self
            .tracker
            .window()
            .is_none_or(|w| !w.matches(&profile.target.window.class, &profile.target.window.title_contains));
        self.profiles.insert(profile.name.clone(), profile);
        if window_changed {
            self.acquire_window();
        }
        self.emergency
            .set_keycodes(release_keycodes(self.profiles.values(), &self.keymap));
        if self.started {
            let profile = self.active_profile();
            match self.origin(&profile).or(new_origin) {
                Ok(origin) => {
                    self.hot.restart(origin);
                    self.hot.set_gate(self.gate(&profile));
                }
                Err(issue) => self.stop(Some(issue.message)),
            }
        }
        Ok(self.revision)
    }

    fn handle(&mut self, cmd: Command) {
        match cmd {
            Command::Connected { client, outbox, writer } => {
                let now = Instant::now();
                self.clients.insert(
                    client,
                    Client {
                        outbox,
                        writer: Some(writer),
                        kind: None,
                        detached: false,
                        telemetry_every: None,
                        live_every: None,
                        last_telemetry: now,
                        last_live: now,
                        trace: false,
                    },
                );
            }
            Command::Request { client, id, request } => {
                let before = self.state();
                let reply = self.request(client, request);
                self.send(client, id, reply);
                let after = self.state();
                if after != before {
                    self.broadcast(Event::State(after));
                }
            }
            Command::Malformed { client, message } => {
                self.send(client, None, Event::error("malformed", message, None));
            }
            Command::Disconnected { client } => {
                if self.picking.as_ref().is_some_and(|p| p.client == client) {
                    self.end_pick(None);
                }
                let removed = self.clients.remove(&client);
                if self.ui_rects.remove(&client).is_some() {
                    self.check_cover();
                }
                self.update_tracing();
                if let Some(c) = removed {
                    // A UI that vanished without detaching: stop watching unless the policy says to
                    // keep running (§4.3). Other clients never stop the engine by leaving (L2).
                    if c.kind == Some(ClientKind::Ui)
                        && !c.detached
                        && self.settings.close_policy != ClosePolicy::KeepRunning
                        && self.started
                    {
                        self.stop(Some("The Glint window closed.".into()));
                        self.broadcast(Event::State(self.state()));
                    }
                }
            }
            Command::Notice(n) => self.notice(n),
            Command::Pick(input) => self.pick_input(input),
            Command::Window(event) => self.on_window(event),
            Command::Desktop(event) => self.on_desktop(event),
            Command::Signal(_) => {
                self.stop(None);
                self.quit = true;
            }
            Command::Hotkey { action, press } => {
                let before = self.state();
                self.hotkey(action, press);
                let after = self.state();
                if after != before {
                    self.broadcast(Event::State(after));
                }
            }
            Command::KeymapChanged => {
                if let Ok(keymap) = Keymap::fetch(&*self.conn) {
                    self.keymap = keymap;
                    self.emergency
                        .set_keycodes(release_keycodes(self.profiles.values(), &self.keymap));
                    self.update_bindings();
                }
            }
        }
    }

    /// Resolves the hotkeys of the current settings on the current keymap (§9).
    pub fn update_bindings(&mut self) {
        let (bindings, problems) = hotkeys::bindings(&self.settings.hotkeys, &self.keymap);
        for p in problems {
            eprintln!("glintd: hotkey {p}");
        }
        self.bindings.store(Arc::new(bindings));
    }

    fn hotkey(&mut self, action: HotkeyAction, press: bool) {
        match (action, press, self.settings.hotkeys.mode) {
            (HotkeyAction::StartStop, true, HotkeyMode::Toggle) => {
                if self.started {
                    self.stop(None);
                } else {
                    self.start_quietly();
                }
            }
            (HotkeyAction::StartStop, true, HotkeyMode::Hold) => self.start_quietly(),
            (HotkeyAction::StartStop, false, HotkeyMode::Hold) => self.stop(None),
            (HotkeyAction::EmergencyStop, true, _) => {
                // The X-events thread has already disarmed the hot loop and released everything.
                self.stop(Some("Emergency stop.".into()));
                self.start_blocked_until = Some(Instant::now() + EMERGENCY_HOLDOFF);
            }
            (HotkeyAction::SampleCenter, true, _) => {
                let event = self.sample_center();
                self.broadcast(event);
            }
            _ => {}
        }
    }

    /// Start from a hotkey: a refusal goes to every client as a warning, since nobody asked.
    fn start_quietly(&mut self) {
        if let Event::Error { code, message, field } = self.start() {
            self.broadcast(Event::Warning { code, message, field });
        }
    }

    /// The color at the watched spot right now, read on connection C (§12.9 *Sample center*).
    /// Starts a color or window pick for `client`: grabs the pointer with a crosshair (§8, §10).
    fn pick(&mut self, client: ClientId, kind: PickKind) -> Event {
        if self.started && kind == PickKind::Color {
            return Event::error("busy", "Stop watching to pick a color.", None);
        }
        if self.picking.as_ref().is_some_and(|p| p.client != client) {
            return Event::error("busy", "Another Glint window is picking a color.", None);
        }
        let cursor = match self.cursor {
            Some(c) => c,
            None => match pick::crosshair(&self.conn) {
                Ok(c) => *self.cursor.insert(c),
                Err(e) => return Event::error("pick", e, None),
            },
        };
        if let Err(e) = pick::grab(&self.conn, self.root, cursor) {
            return Event::error("pick", e, None);
        }
        self.picking = Some(Picking {
            client,
            kind,
            escape: self.keymap.keycode(ESCAPE),
            pending: pick::pointer(&self.conn, self.root),
            last_frame: None,
        });
        Event::PickStarted
    }

    /// Ends the pick, tells its window `event` if there is one, and lets go of the pointer.
    fn end_pick(&mut self, event: Option<Event>) {
        if let Some(p) = self.picking.take() {
            pick::ungrab(&self.conn);
            if let Some(e) = event {
                self.send(p.client, None, e);
            }
        }
    }

    fn pick_input(&mut self, input: PickInput) {
        let Some(p) = self.picking.as_mut() else { return };
        match input {
            PickInput::Moved(x, y) => {
                p.pending = Some((x, y));
                self.pick_frame();
            }
            PickInput::Clicked(1, _, _, top) if p.kind == PickKind::Window => {
                let event = self.window_clicked(top);
                self.end_pick(Some(event));
                self.broadcast(Event::State(self.state()));
            }
            PickInput::Clicked(1, x, y, _) => {
                let event = match pick::around(&self.conn, self.root, &self.root_format, self.screen_size(), (x, y)) {
                    Ok((_, center)) => {
                        let rgb = Rgb::from_u32(center);
                        let lch = Oklch::from_rgb(rgb);
                        Event::ColorPicked {
                            rgb,
                            oklch: [lch.l as f32, lch.c as f32, lch.h as f32],
                        }
                    }
                    Err(e) => Event::error("pick", e, None),
                };
                self.end_pick(Some(event));
            }
            PickInput::Clicked(..) => self.end_pick(Some(Event::PickCancelled)),
            PickInput::Key(k) if Some(k) == p.escape => self.end_pick(Some(Event::PickCancelled)),
            PickInput::Key(_) => {}
        }
    }

    /// Sends a magnifier frame for where the pointer went, at most every [`PICK_FRAME`].
    fn pick_frame(&mut self) {
        let Some(p) = self.picking.as_mut().filter(|p| p.kind == PickKind::Color) else {
            return;
        };
        if p.last_frame.is_some_and(|t| t.elapsed() < PICK_FRAME) {
            return;
        }
        let Some(at) = p.pending.take() else { return };
        p.last_frame = Some(Instant::now());
        let client = p.client;
        if let Ok((pixels, center)) = pick::around(&self.conn, self.root, &self.root_format, self.screen_size(), at) {
            let frame = Event::Magnifier {
                size: pick::SIZE,
                pixels: pixels.into_iter().map(Rgb::from_u32).collect(),
                center: Rgb::from_u32(center),
            };
            self.send(client, None, frame);
        }
    }

    /// The window a window pick's click named: it becomes the active profile's target (§8).
    fn window_clicked(&mut self, top: u32) -> Event {
        if top == x11rb::NONE {
            return Event::error("pick", "That's the desktop: click the game's window.", None);
        }
        let Some(info) = self.tracker.describe_top(&self.conn, top) else {
            return Event::error("pick", "That window can't be watched.", None);
        };
        if info.class.eq_ignore_ascii_case("glint") || info.instance.eq_ignore_ascii_case("glint") {
            return Event::error("pick", "That's Glint's own window: click the game's.", None);
        }
        let class = if info.class.is_empty() {
            info.instance.clone()
        } else {
            info.class.clone()
        };
        if class.is_empty() {
            return Event::error("pick", "That window has no class to recognize it by.", None);
        }
        let mut profile = self.active_profile();
        profile.target.window.class = class.clone();
        profile.target.window.title_contains.clear();
        let title = info.title.clone();
        self.tracker.track(&self.conn, info);
        match self.apply_profile(profile) {
            Ok(_) => Event::WindowPicked { title, class },
            Err(e) => e.into(),
        }
    }

    /// Whether a UI's window covers the watched spot while watching (§8). It would show the
    /// engine its own live swatch, so the swatch stops and the UI is warned.
    fn check_cover(&mut self) {
        let covered = self.started && {
            let control = self.hot.control();
            let (w, h) = self.active_profile().pattern().map_or((1, 1), |p| p.size());
            let spot = Rect::new(i32::from(control.x), i32::from(control.y), w, h);
            self.ui_rects.values().any(|r| r.intersects(&spot))
        };
        if covered == self.covered {
            return;
        }
        self.covered = covered;
        if covered {
            self.broadcast(Event::Warning {
                code: "ui_overlap".into(),
                message: "The Glint window covers the spot it's watching.".into(),
                field: None,
            });
        }
        self.broadcast(Event::State(self.state()));
    }

    /// The trigger's check gap (ADR 0007): four check periods and at least 2 ms; under a
    /// compositor, which shows every frame for a whole refresh, up to most of one frame.
    fn check_gap(&self, profile: &Profile) -> Duration {
        let base = glint_core::TriggerCfg::check_gap(profile.sampling.speed.period());
        if !self.compositor {
            return base;
        }
        let frame = Duration::from_secs_f64(1.0 / self.refresh_hz.unwrap_or(60.0).max(1.0));
        base.max(frame.mul_f64(0.9))
    }

    /// Publishes the active profile again with what the desktop says now (the compositor, the
    /// window), and restarts watching on it.
    fn republish(&mut self) {
        let profile = self.active_profile();
        let Ok(mut compiled) = compile(
            &profile,
            &self.keymap,
            Arc::clone(&self.lut),
            watched_spot(&self.monitors, &profile),
        ) else {
            return;
        };
        compiled.trigger.max_check_gap = self.check_gap(&profile);
        self.hot.set_profile(compiled);
        if self.started {
            match self.origin(&profile) {
                Ok(origin) => self.hot.restart(origin),
                Err(issue) => self.stop(Some(issue.message)),
            }
        }
    }

    fn on_desktop(&mut self, event: DesktopEvent) {
        match event {
            DesktopEvent::Compositor(on) if on != self.compositor => {
                self.compositor = on;
                self.republish();
            }
            DesktopEvent::Locked(true) if self.started && self.active_profile().guards.stop_when_locked => {
                self.stop(Some("Stopped: the screen locked.".into()));
            }
            _ => return,
        }
        self.broadcast(Event::State(self.state()));
    }

    /// Looks for the active profile's window, if it names one.
    fn acquire_window(&mut self) {
        let window = self.active_profile().target.window;
        if window.class.is_empty() && window.title_contains.is_empty() {
            self.tracker.forget();
        } else {
            self.tracker.acquire(&self.conn, &window.class, &window.title_contains);
        }
    }

    /// Whether clicking is allowed now (§8): with *Only click in this window*, only while the
    /// picked window is in front. Without a picked window, screen mode has nothing to compare.
    fn gate(&self, profile: &Profile) -> bool {
        !profile.guards.only_in_window
            || (self.tracker.window().is_none() && profile.target.mode == TargetMode::Screen)
            || self.tracker.in_front()
    }

    /// Why window mode can't start: no window, or it isn't open or showing.
    fn window_blocker(&self, profile: &Profile) -> Option<String> {
        if profile.target.mode != TargetMode::Window || profile.start_blocker().is_some() {
            return None;
        }
        match self.tracker.window() {
            None => Some(format!("{} isn't open.", self.window_title(profile))),
            Some(_) if self.tracker.rect().is_none() => Some(format!("{} is hidden.", self.window_title(profile))),
            Some(_) => None,
        }
    }

    fn window_title(&self, profile: &Profile) -> String {
        match self.tracker.window() {
            Some(w) if !w.title.is_empty() => w.title.clone(),
            _ if !profile.target.window.class.is_empty() => profile.target.window.class.clone(),
            _ => profile.target.window.title_contains.clone(),
        }
    }

    fn window_status(&self, profile: &Profile) -> Option<WindowStatus> {
        let w = &profile.target.window;
        if w.class.is_empty() && w.title_contains.is_empty() {
            return None;
        }
        Some(WindowStatus {
            title: self.window_title(profile),
            state: self.tracker.status().unwrap_or(WindowState::Closed),
        })
    }

    /// A window event: follow the window, and pause, restart or stop watching as it says (§8).
    fn on_window(&mut self, event: WinEvent) {
        let before = self.window_status(&self.active_profile());
        let change = self.tracker.event(&self.conn, event);
        let profile = self.active_profile();
        let window_mode = profile.target.mode == TargetMode::Window;
        match change {
            Change::None => return,
            Change::Appeared => {
                if self.tracker.window().is_none() {
                    self.acquire_window();
                }
            }
            Change::Moved if self.started && window_mode => match self.origin(&profile) {
                // A move can't click: the restart latches (R7).
                Ok(origin) => {
                    self.hot.restart(origin);
                    self.check_cover();
                }
                Err(issue) => self.stop(Some(issue.message)),
            },
            Change::Moved => {}
            Change::Closed if self.started && window_mode => {
                let title = before.as_ref().map(|b| b.title.clone()).unwrap_or_default();
                self.stop(Some(format!(
                    "{title} closed. Glint will find it again when it reopens."
                )));
            }
            Change::Status if self.started && window_mode && self.tracker.rect().is_none() => {
                let title = self.window_title(&profile);
                self.stop(Some(format!("{title} was hidden.")));
            }
            Change::Closed | Change::Status => {}
        }
        if self.started {
            self.hot.set_gate(self.gate(&profile));
        }
        if self.window_status(&profile) != before || change == Change::Closed {
            self.broadcast(Event::State(self.state()));
        }
    }

    fn screen_size(&self) -> (u16, u16) {
        self.conn
            .setup()
            .roots
            .get(usize::from(self.display.screen))
            .map_or((0, 0), |s| (s.width_in_pixels, s.height_in_pixels))
    }

    pub fn sample_center(&self) -> Event {
        let profile = self.active_profile();
        let (x, y) = watched_spot(&self.monitors, &profile);
        let reply = self
            .conn
            .get_image(ImageFormat::Z_PIXMAP, self.root, x, y, 1, 1, u32::MAX)
            .map_err(|e| e.to_string())
            .and_then(|c| c.reply().map_err(|e| e.to_string()));
        let mut px = [0u32; 1];
        match reply.map(|r| self.root_format.decode(&r.data, 1, 1, &mut px)) {
            Ok(Ok(())) => {
                let rgb = Rgb::from_u32(px[0]);
                let lch = Oklch::from_rgb(rgb);
                Event::ColorSampled {
                    rgb,
                    oklch: [lch.l as f32, lch.c as f32, lch.h as f32],
                }
            }
            Ok(Err(e)) => Event::error("sample", e.to_string(), None),
            Err(e) => Event::error("sample", e, None),
        }
    }

    fn request(&mut self, client: ClientId, request: Request) -> Event {
        match request {
            Request::Hello {
                protocol, client: kind, ..
            } => {
                if protocol != PROTOCOL {
                    return Event::error(
                        "protocol",
                        format!("This engine speaks protocol {PROTOCOL}, not {protocol}. Restart the engine."),
                        None,
                    );
                }
                if let Some(c) = self.clients.get_mut(&client) {
                    c.kind = Some(kind);
                }
                Event::HelloAck {
                    protocol: PROTOCOL,
                    engine_version: env!("CARGO_PKG_VERSION").into(),
                    caps: CapsInfo {
                        xtest: self.caps.xtest.to_string(),
                        shm: self.caps.shm.is_some(),
                        damage: self.caps.damage.is_some(),
                        composite: self.caps.composite.is_some(),
                        randr: self.caps.randr.map(|v| v.to_string()),
                        xi2: self.caps.xinput.map(|v| v.to_string()),
                    },
                    session: SessionInfo {
                        display: self.display.canonical(),
                        xwayland: self.caps.xwayland || glint_x11::env::wayland_session(),
                        compositor: self.compositor.then(|| "running".into()),
                        realtime: "off".into(),
                    },
                }
            }
            Request::GetState => Event::State(self.state()),
            Request::Subscribe {
                telemetry_hz,
                live_hz,
                trace,
            } => {
                let every = |hz: u8, max: u8| (hz > 0).then(|| Duration::from_secs(1) / u32::from(hz.min(max)));
                if let Some(c) = self.clients.get_mut(&client) {
                    c.telemetry_every = every(telemetry_hz, 10);
                    c.live_every = every(live_hz, 20);
                    c.trace = trace;
                }
                self.update_tracing();
                Event::State(self.state())
            }
            Request::SetProfile { profile } => match self.apply_profile(*profile) {
                Ok(rev) => Event::ProfileApplied { rev },
                Err(e) => e.into(),
            },
            Request::GetProfile { name } => {
                let name = name.unwrap_or_else(|| self.active.clone());
                match self.profiles.get(&name) {
                    Some(p) => Event::Profile {
                        profile: Box::new(p.clone()),
                    },
                    None => Event::error("no_profile", format!("There's no profile named {name:?}."), None),
                }
            }
            Request::UiGeometry { x, y, w, h } => {
                self.ui_rects.insert(client, Rect::new(x, y, w, h));
                self.check_cover();
                Event::State(self.state())
            }
            Request::PickColor => self.pick(client, PickKind::Color),
            Request::PickWindow => self.pick(client, PickKind::Window),
            Request::CancelPick => {
                if self.picking.as_ref().is_some_and(|p| p.client == client) {
                    self.end_pick(None);
                }
                Event::PickCancelled
            }
            Request::GetSettings => Event::Settings {
                settings: Box::new(self.settings.clone()),
            },
            Request::UseProfile { name } => match self.profiles.get(&name).cloned() {
                Some(p) => match self.apply_profile(p) {
                    Ok(_) => Event::State(self.state()),
                    Err(e) => e.into(),
                },
                None => Event::error("no_profile", format!("There's no profile named {name:?}."), None),
            },
            Request::ListProfiles => Event::Profiles {
                names: self.profiles.keys().cloned().collect(),
                active: self.active.clone(),
            },
            Request::SetSettings { settings } => {
                let report = validate::settings(&settings);
                let keys = validate::action_against_hotkeys(&self.active_profile().action, &settings.hotkeys);
                match report.errors.into_iter().chain(keys.errors).next() {
                    Some(issue) => Event::error("invalid", issue.message, Some(issue.field)),
                    None => {
                        let mut settings = *settings;
                        // The active profile follows use_profile and set_profile, not this request.
                        settings.active_profile = self.active.clone();
                        self.store.save_settings(&settings, Instant::now());
                        self.settings = settings;
                        self.update_bindings();
                        Event::SettingsApplied
                    }
                }
            }
            Request::Start => self.start(),
            Request::Stop => {
                self.stop(None);
                Event::State(self.state())
            }
            Request::Toggle => {
                if self.started {
                    self.stop(None);
                    Event::State(self.state())
                } else {
                    self.start()
                }
            }
            Request::EmergencyStop => {
                self.stop(Some("Emergency stop.".into()));
                let _ = self.emergency.release_everything();
                self.start_blocked_until = Some(Instant::now() + EMERGENCY_HOLDOFF);
                Event::State(self.state())
            }
            Request::ReleaseAll => match self.emergency.release_everything() {
                Ok(()) => Event::State(self.state()),
                Err(e) => Event::error("x11", e.to_string(), None),
            },
            Request::SampleCenter => self.sample_center(),
            Request::Classify { colors } => {
                if colors.len() > MAX_CLASSIFY {
                    return Event::error("too_many", format!("At most {MAX_CLASSIFY} colors at once."), None);
                }
                Event::Classified {
                    verdicts: colors.iter().map(|c| self.lut.class(c.to_u32())).collect(),
                }
            }
            Request::Detach => {
                if let Some(c) = self.clients.get_mut(&client) {
                    c.detached = true;
                }
                Event::Bye
            }
            Request::Quit => {
                self.stop(None);
                self.quit = true;
                Event::Bye
            }
            Request::Ping => Event::Pong,
        }
    }

    fn notice(&mut self, n: Notice) {
        match n {
            Notice::Fired { count, latency } => {
                self.fired = Some((count, u32::try_from(latency.as_micros()).unwrap_or(u32::MAX)));
            }
            Notice::Stopped(reason) => {
                let why = match reason {
                    StopReason::SafetyStop => format!(
                        "Stopped: {} clicks in 1 s — looks like a loop.",
                        self.active_profile().trigger.max_clicks_per_sec + 1
                    ),
                    StopReason::TargetGone => "The watched window was hidden or closed.".into(),
                    StopReason::CaptureFailed => "Reading the screen failed.".into(),
                    StopReason::InputFailed => "Sending input failed.".into(),
                };
                if self.started {
                    self.started = false;
                    self.last_stop = Some(why);
                    self.broadcast(Event::State(self.state()));
                }
            }
            Notice::MaybeMissed | Notice::Exited => {}
            Notice::Trace(t) => {
                let entry = trace_entry(t);
                let tracers: Vec<ClientId> = self
                    .clients
                    .iter()
                    .filter(|(_, c)| c.trace)
                    .map(|(id, _)| *id)
                    .collect();
                for id in tracers {
                    self.send(id, None, Event::Trace(entry));
                }
            }
        }
    }

    /// Tells the telemetry thread whether anyone wants the trace stream.
    fn update_tracing(&self) {
        let any = self.clients.values().any(|c| c.trace);
        self.hot.shared().tracing.store(any, Ordering::Relaxed);
    }

    fn tick(&mut self) {
        self.hot.free_retired();
        self.pick_frame();
        let now = Instant::now();
        for problem in self.store.flush_due(now) {
            eprintln!("glintd: saving {problem}");
        }
        if let Some((count, latency_us)) = self.fired
            && now.duration_since(self.last_fired) >= FIRED_EVERY
        {
            self.fired = None;
            self.last_fired = now;
            // Only to clients that subscribed: one that never reads must not fill up with these.
            let subscribed: Vec<ClientId> = self
                .clients
                .iter()
                .filter(|(_, c)| c.telemetry_every.is_some() || c.live_every.is_some())
                .map(|(id, _)| *id)
                .collect();
            for id in subscribed {
                self.send(id, None, Event::Fired { count, latency_us });
            }
        }
        let wants_telemetry = self
            .clients
            .values()
            .any(|c| c.telemetry_every.is_some_and(|e| now - c.last_telemetry >= e));
        let telemetry = wants_telemetry.then(|| {
            let s = self.stats.lock().map(|s| s.clone()).unwrap_or_default();
            Event::Telemetry(TelemetrySnapshot {
                checks_per_sec: s.checks_per_sec,
                capture_p50_us: s.capture_p50_us,
                capture_p99_us: s.capture_p99_us,
                decide_p50_us: s.decide_p50_us,
                clicks: s.clicks,
                suppressed: s.suppressed,
                maybe_missed: s.maybe_missed,
                stalls: s.stalls,
                worst_stall_ms: s.worst_stall_ms,
                dropped: s.dropped,
            })
        });
        let preview = *self.preview.read();
        // Only while watching: a stopped engine's last frame is stale, and sending it would wake
        // the window for nothing.
        let live = preview
            .verdict
            .filter(|_| self.started && !self.covered)
            .map(|verdict| Event::Live {
                rgb: Rgb::from_u32(preview.rgb),
                verdict,
            });
        let ids: Vec<ClientId> = self.clients.keys().copied().collect();
        for id in ids {
            let Some(c) = self.clients.get_mut(&id) else { continue };
            let mut out = Vec::new();
            if let (Some(every), Some(t)) = (c.telemetry_every, &telemetry)
                && now - c.last_telemetry >= every
            {
                c.last_telemetry = now;
                out.push(t.clone());
            }
            if let (Some(every), Some(l)) = (c.live_every, &live)
                && now - c.last_live >= every
            {
                c.last_live = now;
                out.push(l.clone());
            }
            for e in out {
                self.send(id, None, e);
            }
        }
    }

    fn send(&mut self, client: ClientId, id: Option<u64>, event: Event) {
        let Some(c) = self.clients.get(&client) else { return };
        let droppable = matches!(
            event,
            Event::Telemetry(_) | Event::Live { .. } | Event::Fired { .. } | Event::Magnifier { .. }
        );
        match c.outbox.try_send(glint_ipc::encode(&Envelope::new(id, event))) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) if droppable => {}
            Err(_) => {
                // Not reading, or gone: dropping the queue ends its writer and closes the socket.
                self.clients.remove(&client);
                self.update_tracing();
            }
        }
    }

    fn broadcast(&mut self, event: Event) {
        let ids: Vec<ClientId> = self.clients.keys().copied().collect();
        for id in ids {
            self.send(id, None, event.clone());
        }
    }
}
