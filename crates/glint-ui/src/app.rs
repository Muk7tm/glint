// SPDX-License-Identifier: GPL-3.0-or-later
//! The window's logic (PLAN §12). It keeps the engine's profile, settings and state, shows them,
//! and turns what the user does into requests. All of it runs on the window's event loop; the
//! engine's messages arrive there through [`deliver`].

use std::cell::RefCell;
use std::time::Duration;

use glint_core::settings::ClosePolicy;
use glint_core::{Profile, Rgb, Settings, Verdict};
use glint_ipc::{Event, Phase, Request, StateSnapshot};
use glint_ui_view::{AppWindow, Form, Tone, Ui};
use slint::ComponentHandle;

use glint_core::profile::{ActionKind, KeySpec};

use crate::bind::{self, Fields};
use crate::client::{FromEngine, Link};
use crate::keys::{self, Resolver};

/// How long a message stays in the hero card's message line.
const TOAST: Duration = Duration::from_secs(5);
/// The live swatch's rate while the window can be seen.
const LIVE_HZ: u8 = 10;
/// How long the window waits for its last request to go out when it closes.
const LAST_WORD: Duration = Duration::from_secs(1);

pub struct App {
    ui: slint::Weak<AppWindow>,
    link: Link,
    connected: bool,
    /// The link is starting or reaching the engine.
    starting: bool,
    /// Why there's no engine, while there isn't one.
    offline: String,
    /// The active profile and the settings, as the engine has them (defaults until it says).
    profile: Profile,
    settings: Settings,
    state: StateSnapshot,
    shown: Fields,
    /// *Keyboard key* is chosen but has no key yet: nothing is sent until it has one.
    choosing_key: bool,
    keys: Resolver,
    toast: slint::Timer,
    /// While minimized the live swatch is off, and this checks once a second for the window
    /// coming back (Slint has no event for it), so a hidden window costs about nothing.
    hidden: slint::Timer,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Runs `f` on the app, if it has been installed on this thread.
pub fn with(f: impl FnOnce(&mut App)) {
    APP.with(|app| {
        if let Some(app) = app.borrow_mut().as_mut() {
            f(app);
        }
    });
}

/// Hands a message from the link's threads to the window's event loop.
pub fn deliver(message: FromEngine) {
    let _ = slint::invoke_from_event_loop(move || with(|app| app.on_engine(message)));
}

impl App {
    /// Installs the app for `ui` on this (the event loop's) thread.
    pub fn install(ui: &AppWindow, link: Link) {
        let app = Self {
            ui: ui.as_weak(),
            link,
            connected: false,
            starting: true,
            offline: "Starting Glint's engine…".into(),
            profile: Profile::default(),
            settings: Settings::default(),
            state: StateSnapshot::default(),
            shown: Fields::default(),
            choosing_key: false,
            keys: Resolver::default(),
            toast: slint::Timer::default(),
            hidden: slint::Timer::default(),
        };
        APP.with(|slot| *slot.borrow_mut() = Some(app));
        with(Self::refresh);
    }

    /// Shows the profile, settings and state.
    fn refresh(&mut self) {
        let Some(ui) = self.ui.upgrade() else { return };
        self.shown = bind::fields(&self.profile, &self.settings);
        if self.choosing_key {
            self.shown.click = bind::KEYBOARD_KEY;
        }
        let mut hero = bind::hero(&self.shown, self.state.started, self.state.cannot_start.as_deref());
        if !self.connected {
            // Without an engine, the hero button starts one (§4.3).
            hero = bind::Hero {
                button: "↻  Restart engine".into(),
                enabled: !self.starting,
                danger: false,
                hint: self.offline.clone(),
            };
        }
        bind::apply(&ui, &self.shown, &hero);
        let hint = if self.choosing_key {
            "Press a key, or pick one from the list."
        } else {
            ""
        };
        ui.global::<Form>().set_key_hint(hint.into());
        ui.global::<Form>().set_editable(self.connected);
        ui.global::<Ui>().set_connected(self.connected);
        if !self.watching() {
            self.not_seeing(&ui);
        }
    }

    fn watching(&self) -> bool {
        self.connected && self.state.started && self.state.phase == Phase::Watching
    }

    fn not_seeing(&self, ui: &AppWindow) {
        let form = ui.global::<Form>();
        form.set_seeing_known(false);
        form.set_seeing_text("Not watching".into());
    }

    /// Shows `text` in the hero card's message line for a few seconds.
    pub fn toast(&mut self, text: &str, tone: Tone) {
        let Some(ui) = self.ui.upgrade() else { return };
        let state = ui.global::<Ui>();
        state.set_toast(text.into());
        state.set_toast_tone(tone);
        let weak = self.ui.clone();
        self.toast.start(slint::TimerMode::SingleShot, TOAST, move || {
            if let Some(ui) = weak.upgrade() {
                ui.global::<Ui>().set_toast("".into());
            }
        });
    }

    fn on_engine(&mut self, message: FromEngine) {
        match message {
            FromEngine::Connected { .. } => {
                self.connected = true;
                self.starting = false;
                for request in [
                    Request::Subscribe {
                        telemetry_hz: 0,
                        live_hz: LIVE_HZ,
                        trace: false,
                    },
                    Request::GetState,
                    Request::GetProfile { name: None },
                    Request::GetSettings,
                ] {
                    self.link.send(request);
                }
            }
            FromEngine::Disconnected(why) => {
                self.connected = false;
                self.starting = false;
                self.offline = why;
                self.state = StateSnapshot::default();
            }
            FromEngine::Event(event) => self.on_event(*event),
        }
        self.refresh();
    }

    fn on_event(&mut self, event: Event) {
        match event {
            Event::State(s) => {
                if s.last_stop.is_some() && s.last_stop != self.state.last_stop && !s.started {
                    let why = s.last_stop.clone().unwrap_or_default();
                    self.toast(&why, Tone::Warning);
                }
                if s.profile != self.state.profile && !s.profile.is_empty() {
                    self.link.send(Request::GetProfile { name: None });
                }
                self.state = s;
            }
            Event::Profile { profile } => self.profile = *profile,
            Event::Settings { settings } => self.settings = *settings,
            Event::Live { rgb, verdict } if self.watching() => {
                if self.ui.upgrade().is_some_and(|ui| ui.window().is_minimized()) {
                    self.hide_live();
                } else {
                    self.seeing(rgb, verdict);
                }
            }
            Event::Error { message, .. } => {
                self.set_picking(false);
                self.toast(&message, Tone::Error);
                // What the window sent was refused: show what the engine really has.
                self.link.send(Request::GetProfile { name: None });
                self.link.send(Request::GetSettings);
            }
            Event::Warning { message, .. } => self.toast(&message, Tone::Warning),
            Event::PickStarted => self.set_picking(true),
            Event::Magnifier { size, pixels, .. } => self.magnifier(size, &pixels),
            Event::ColorPicked { rgb, .. } => {
                self.set_picking(false);
                self.set_color(rgb);
            }
            Event::PickCancelled => self.set_picking(false),
            Event::ColorSampled { rgb, .. } => {
                self.set_color(rgb);
                self.toast(&format!("Sampled {rgb} at the watched spot."), Tone::Neutral);
            }
            _ => {}
        }
    }

    /// The *Seeing* row: the color at the watched spot right now and what it counts as.
    fn seeing(&self, rgb: Rgb, verdict: Verdict) {
        let Some(ui) = self.ui.upgrade() else { return };
        let form = ui.global::<Form>();
        form.set_seeing_known(true);
        form.set_seeing_color(slint::Color::from_rgb_u8(rgb.r(), rgb.g(), rgb.b()));
        let what = match verdict {
            Verdict::Enter => "match",
            Verdict::Present => "close match",
            Verdict::Absent => "no match",
        };
        form.set_seeing_text(format!("{rgb} · {what}").into());
    }

    /// The window is minimized: stop the live swatch until it's back.
    fn hide_live(&mut self) {
        if self.hidden.running() {
            return;
        }
        self.link.send(live(0));
        let (weak, link) = (self.ui.clone(), self.link.clone());
        self.hidden
            .start(slint::TimerMode::Repeated, Duration::from_secs(1), move || {
                if weak.upgrade().is_some_and(|ui| !ui.window().is_minimized()) {
                    link.send(live(LIVE_HZ));
                    with(|app| app.hidden.stop());
                }
            });
    }

    /// The window manager's close button or *Quit Glint*, by the close policy (§12.5).
    pub fn close_requested(&mut self) {
        match self.settings.close_policy {
            ClosePolicy::Ask if self.connected => {
                if let Some(ui) = self.ui.upgrade() {
                    ui.invoke_ask_before_closing();
                }
            }
            ClosePolicy::Ask => self.leave(None),
            ClosePolicy::KeepRunning => self.keep_running(),
            ClosePolicy::Quit => self.quit(),
        }
    }

    /// Closes the window and leaves the engine running, so the hotkeys keep working.
    pub fn keep_running(&mut self) {
        self.leave(Some(Request::Detach));
    }

    /// Quits the engine too: it stops, releases input, and exits.
    pub fn quit(&mut self) {
        self.leave(Some(Request::Quit));
    }

    fn leave(&mut self, last: Option<Request>) {
        if let Some(request) = last.filter(|_| self.connected) {
            self.link.finish(request, LAST_WORD);
        }
        let _ = slint::quit_event_loop();
    }

    /// The hero button: Start, Stop, or restarting a missing engine.
    pub fn main_button(&mut self) {
        if !self.connected {
            self.starting = true;
            self.offline = "Starting Glint's engine…".into();
            self.link.restart();
            self.refresh();
            return;
        }
        self.link.send(if self.state.started {
            Request::Stop
        } else {
            Request::Start
        });
    }

    /// The user changed a setting: send what changed.
    pub fn form_changed(&mut self) {
        let Some(ui) = self.ui.upgrade() else { return };
        let edits = bind::read(&ui, &self.shown);
        let (mut profile, settings) = bind::edited(&self.profile, &self.settings, &edits);
        // A key action needs its key first.
        self.choosing_key = profile.action.kind == ActionKind::Key && profile.action.key.is_none();
        if self.choosing_key {
            profile.action = self.profile.action.clone();
        }
        if profile != self.profile {
            self.link.send(Request::SetProfile {
                profile: Box::new(profile.clone()),
            });
            self.profile = profile;
        }
        if settings != self.settings {
            self.link.send(Request::SetSettings {
                settings: Box::new(settings.clone()),
            });
            self.settings = settings;
        }
        self.refresh();
    }
}

impl App {
    /// A key pressed in the *Key to press* picker.
    pub fn key_captured(&mut self, text: &str) {
        match keys::name_from_slint(text) {
            Some(name) => self.key_chosen(&name),
            None => self.toast("Glint can't press that key; pick one from the list.", Tone::Warning),
        }
    }

    /// The key to press, by name.
    pub fn key_chosen(&mut self, name: &str) {
        let Some(keycode) = self.keys.keycode(name) else {
            return self.toast(&format!("This keyboard has no {name} key."), Tone::Warning);
        };
        let mut profile = self.profile.clone();
        profile.action.kind = ActionKind::Key;
        profile.action.key = Some(KeySpec {
            keycode,
            name: name.to_owned(),
        });
        self.choosing_key = false;
        self.link.send(Request::SetProfile {
            profile: Box::new(profile.clone()),
        });
        self.profile = profile;
        self.refresh();
    }

    /// A key pressed in the *Start/stop key* picker.
    pub fn hotkey_captured(&mut self, text: &str) {
        match keys::name_from_slint(text) {
            Some(name) => self.hotkey_chosen(&name),
            None => self.toast("That key can't be a hotkey; pick one from the list.", Tone::Warning),
        }
    }

    /// The start/stop hotkey, by name. The engine checks it (§9) and says why if it refuses.
    pub fn hotkey_chosen(&mut self, name: &str) {
        let mut settings = self.settings.clone();
        settings.hotkeys.start_stop = name.to_owned();
        self.link.send(Request::SetSettings {
            settings: Box::new(settings.clone()),
        });
        self.settings = settings;
        self.refresh();
    }
}

impl App {
    /// *Pick*: the engine grabs the pointer and streams the magnifier.
    pub fn pick_color(&mut self) {
        if self.state.started {
            return self.toast("Stop watching to pick a color.", Tone::Warning);
        }
        self.link.send(Request::PickColor);
    }

    /// A color typed as a hex code.
    pub fn hex_entered(&mut self, text: &str) {
        match keys::parse_hex(text) {
            Some(rgb) => self.set_color(rgb),
            None => {
                self.toast("Type a color as a hex code, like #3BD16F.", Tone::Warning);
                self.refresh();
            }
        }
    }

    pub fn cancel_pick(&mut self) {
        self.link.send(Request::CancelPick);
    }

    fn set_picking(&self, on: bool) {
        if let Some(ui) = self.ui.upgrade() {
            let form = ui.global::<Form>();
            form.set_picking_color(on);
            if !on {
                form.set_magnifier_ready(false);
            }
        }
    }

    fn magnifier(&self, size: u16, pixels: &[Rgb]) {
        let Some(ui) = self.ui.upgrade() else { return };
        let n = u32::from(size);
        if pixels.len() != (n * n) as usize {
            return;
        }
        let mut buffer = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(n, n);
        for (out, rgb) in buffer.make_mut_slice().iter_mut().zip(pixels) {
            *out = slint::Rgb8Pixel::new(rgb.r(), rgb.g(), rgb.b());
        }
        let form = ui.global::<Form>();
        form.set_magnifier(slint::Image::from_rgb8(buffer));
        form.set_magnifier_ready(true);
    }

    /// Makes `rgb` the color to watch for, with the shades chosen now.
    fn set_color(&mut self, rgb: Rgb) {
        let preset = bind::shade_preset(self.shown.shades);
        let mut profile = self.profile.clone();
        let entry = preset.entry(profile.color.model, rgb);
        match profile.color.include.first_mut() {
            Some(first) => *first = entry,
            None => profile.color.include.push(entry),
        }
        self.link.send(Request::SetProfile {
            profile: Box::new(profile.clone()),
        });
        self.profile = profile;
        self.refresh();
    }
}

fn live(hz: u8) -> Request {
    Request::Subscribe {
        telemetry_hz: 0,
        live_hz: hz,
        trace: false,
    }
}
