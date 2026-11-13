// SPDX-License-Identifier: MIT
//! More options → ENGINE (PLAN §12.4): live numbers, the latency test, profiles, diagnostics, and
//! importing or exporting a profile through the desktop's file dialog.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use glint_core::Profile;
use glint_ipc::{Request, TelemetrySnapshot};
use glint_ui_view::{Form, Tone, Ui};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use super::{App, LIVE_HZ, with};
use crate::bind::thousands;
use crate::portal::{self, Choose};
use crate::{autostart, spawn};

/// Appearances in the latency test: about a minute in a test X server.
const LATENCY_RUNS: u32 = 200;
/// Telemetry while *More options* is open.
const TELEMETRY_HZ: u8 = 2;

impl App {
    /// The subscription for the window as it is: telemetry only while *More options* is open.
    pub(super) fn subscription(&self, live_hz: u8) -> Request {
        Request::Subscribe {
            telemetry_hz: if self.settings.ui.more_options_open {
                TELEMETRY_HZ
            } else {
                0
            },
            live_hz,
            trace: false,
        }
    }

    /// *More options* opened or closed: remembered, and the telemetry follows.
    pub fn more_toggled(&mut self) {
        let Some(ui) = self.ui.upgrade() else { return };
        let open = ui.global::<Ui>().get_more_open();
        if open != self.settings.ui.more_options_open {
            self.settings.ui.more_options_open = open;
            self.link.send(Request::SetSettings {
                settings: Box::new(self.settings.clone()),
            });
        }
        self.link.send(self.subscription(LIVE_HZ));
        if open {
            self.link.send(Request::ListProfiles);
            // The entry may have changed outside Glint, in the desktop's own autostart settings.
            self.show_start_at_login();
        }
    }

    /// Whether the engine starts at login, as the autostart entry says.
    pub(super) fn show_start_at_login(&self) {
        let Some(ui) = self.ui.upgrade() else { return };
        let on = autostart::entry_path().is_some_and(|entry| autostart::is_on(&entry));
        ui.global::<Form>().set_start_at_login(on);
    }

    /// *Start the engine when I log in* turned on or off: the autostart entry follows, and runs
    /// the same engine this window starts.
    pub fn start_at_login_toggled(&mut self) {
        let Some(ui) = self.ui.upgrade() else { return };
        let form = ui.global::<Form>();
        let on = form.get_start_at_login();
        let result = match autostart::entry_path() {
            Some(entry) => autostart::set(&entry, on, &spawn::engine_path()).map_err(|e| e.to_string()),
            None => Err("no home folder".to_owned()),
        };
        match result {
            Ok(()) if on => self.toast("Glint's engine will start when you log in.", Tone::Neutral),
            Ok(()) => self.toast("Glint's engine won't start when you log in.", Tone::Neutral),
            Err(e) => {
                form.set_start_at_login(!on);
                self.toast(&format!("Couldn't change starting at login: {e}."), Tone::Error);
            }
        }
    }

    /// The settings arrived: *More options* opens as it was left.
    pub(super) fn settings_arrived(&mut self) {
        if let Some(ui) = self.ui.upgrade() {
            ui.global::<Ui>().set_more_open(self.settings.ui.more_options_open);
        }
        self.link.send(self.subscription(LIVE_HZ));
    }

    pub(super) fn telemetry(&self, t: &TelemetrySnapshot) {
        let Some(ui) = self.ui.upgrade() else { return };
        let text = if t.checks_per_sec < 1.0 {
            "not watching".to_owned()
        } else {
            format!(
                "{}/sec · read p50 {:.2} ms, p99 {:.2} ms · {} clicks",
                thousands(t.checks_per_sec.round() as u32),
                t.capture_p50_us / 1000.0,
                t.capture_p99_us / 1000.0,
                thousands(u32::try_from(t.clicks).unwrap_or(u32::MAX)),
            )
        };
        ui.global::<Form>().set_response_text(text.into());
    }

    /// The real-time line, from the engine's state.
    pub(super) fn show_engine(&self) {
        let Some(ui) = self.ui.upgrade() else { return };
        let text = match (&self.state.realtime, &self.state.cpu) {
            (Some(rt), Some(cpu)) if self.connected => format!("{rt} · runs on {cpu}"),
            _ => String::new(),
        };
        ui.global::<Form>().set_realtime_text(text.into());
    }

    pub(super) fn profiles(&mut self, names: Vec<String>, active: &str) {
        let Some(ui) = self.ui.upgrade() else { return };
        let form = ui.global::<Form>();
        let index = names.iter().position(|n| n == active).map_or(-1, |i| i as i32);
        let model: Vec<SharedString> = names.iter().map(SharedString::from).collect();
        form.set_profile_names(ModelRc::new(VecModel::from(model)));
        form.set_profile_index(index);
        self.profile_names = names;
    }

    pub fn use_profile(&mut self, i: i32) {
        if let Some(name) = usize::try_from(i).ok().and_then(|i| self.profile_names.get(i)) {
            self.link.send(Request::UseProfile { name: name.clone() });
        }
    }

    /// *Copy diagnostics*: the engine's text block, then the window's own lines.
    pub fn copy_diagnostics(&mut self) {
        self.link.send(Request::Diagnostics);
    }

    pub(super) fn diagnostics(&mut self, text: &str) {
        let Some(ui) = self.ui.upgrade() else { return };
        // Slint is pinned to =1.18.1 in Cargo.toml.
        let all = format!("{text}Window: glint {} · Slint 1.18.1\n", env!("CARGO_PKG_VERSION"));
        ui.global::<Ui>().set_diagnostics(all.into());
        ui.invoke_copy_diagnostics();
        self.toast("Copied the diagnostics to the clipboard.", Tone::Neutral);
    }

    /// *Import*: a profile file, which becomes the active profile.
    pub fn import_profile(&mut self) {
        std::thread::spawn(|| {
            let result = portal::choose(&Choose::Open).and_then(|path| match path {
                None => Ok(None),
                Some(path) => {
                    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                    let profile = Profile::load(&text).map_err(|e| format!("{}: {e}", path.display()))?;
                    Ok(Some(profile.value))
                }
            });
            let _ = slint::invoke_from_event_loop(move || with(|a| a.imported(result)));
        });
    }

    fn imported(&mut self, result: Result<Option<Profile>, String>) {
        match result {
            Ok(None) => {}
            Ok(Some(profile)) => {
                let name = profile.name.clone();
                self.send_profile(profile);
                self.link.send(Request::ListProfiles);
                self.toast(&format!("Imported {name}; it's the active profile."), Tone::Neutral);
            }
            Err(why) => self.toast(&format!("Couldn't import: {why}"), Tone::Error),
        }
    }

    /// *Export*: the active profile to a file.
    pub fn export_profile(&mut self) {
        let profile = self.profile.clone();
        let name = format!("{}.toml", file_name(&profile.name));
        std::thread::spawn(move || {
            let result = portal::choose(&Choose::Save { name }).and_then(|path| match path {
                None => Ok(None),
                Some(path) => {
                    let text = profile.to_toml().map_err(|e| e.to_string())?;
                    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
                    Ok(Some(path))
                }
            });
            let _ = slint::invoke_from_event_loop(move || with(|a| a.exported(result)));
        });
    }

    fn exported(&mut self, result: Result<Option<PathBuf>, String>) {
        match result {
            Ok(None) => {}
            Ok(Some(path)) => self.toast(&format!("Exported to {}.", path.display()), Tone::Neutral),
            Err(why) => self.toast(&format!("Couldn't export: {why}"), Tone::Error),
        }
    }

    /// *Run latency test*: `glint-lab basic` in a test X server of its own (ADR 0019), which
    /// never touches this desktop.
    pub fn latency_test(&mut self) {
        let Some(ui) = self.ui.upgrade() else { return };
        let form = ui.global::<Form>();
        form.set_latency_running(true);
        form.set_latency_text("".into());
        let lab = std::env::current_exe()
            .ok()
            .map(|p| p.with_file_name("glint-lab"))
            .filter(|p| p.exists())
            .unwrap_or_else(|| PathBuf::from("glint-lab"));
        std::thread::spawn(move || {
            let result = Command::new(&lab)
                .args(["basic", "--count", &LATENCY_RUNS.to_string()])
                .stdin(Stdio::null())
                .output()
                .map_err(|e| format!("{}: {e}", lab.display()))
                .and_then(|out| {
                    let text = String::from_utf8_lossy(&out.stdout);
                    summarize(&text).ok_or_else(|| {
                        let err = String::from_utf8_lossy(&out.stderr);
                        err.lines().last().unwrap_or("the test didn't finish").to_owned()
                    })
                });
            let _ = slint::invoke_from_event_loop(move || with(|a| a.latency_done(result)));
        });
    }

    fn latency_done(&mut self, result: Result<String, String>) {
        let Some(ui) = self.ui.upgrade() else { return };
        let form = ui.global::<Form>();
        form.set_latency_running(false);
        match result {
            Ok(text) => form.set_latency_text(text.into()),
            Err(why) => {
                form.set_latency_text("".into());
                self.toast(&format!("The latency test didn't run: {why}"), Tone::Error);
            }
        }
    }
}

/// The latency test's line from `glint-lab`'s report: runs, extra and missed clicks, and the
/// response at p50 and p99.
pub fn summarize(report: &str) -> Option<String> {
    let line = |prefix: &str| report.lines().find(|l| l.starts_with(prefix));
    let counts = line("appearances ")?;
    let number = |label: &str| {
        counts
            .split(" · ")
            .find_map(|part| part.strip_prefix(label))
            .map(str::trim)
            .and_then(|n| n.parse::<u32>().ok())
    };
    let (runs, extra, missed) = (number("appearances ")?, number("extra ")?, number("missed ")?);
    let mut text = format!("last: {} runs · {extra} extra · {missed} missed", thousands(runs));
    if let Some(response) = line("response") {
        let ms = |label: &str| {
            response
                .split(" · ")
                .find_map(|part| part.trim().trim_start_matches("response").trim().strip_prefix(label))
                .map(|v| v.trim().trim_end_matches(" ms").to_owned())
        };
        if let (Some(p50), Some(p99)) = (ms("p50 "), ms("p99 ")) {
            text.push_str(&format!(" · p50 {p50} · p99 {p99} ms"));
        }
    }
    Some(text)
}

/// A file name for a profile name: letters, digits, spaces, `-` and `_` stay.
fn file_name(name: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || " -_".contains(c) {
                c
            } else {
                '-'
            }
        })
        .collect();
    let clean = clean.trim();
    if clean.is_empty() {
        "glint-profile".into()
    } else {
        clean.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_latency_line_from_a_lab_report() {
        let report = "glint-lab basic --count 200 · speed balanced · backend screen\n\
            appearances 200 · clicks 200 · extra 0 · missed 0 · possible misses reported 0\n\
            response    p50 0.15 ms · p95 0.25 ms · p99 0.31 ms · max 0.40 ms\n\
            verdict     PASS\n";
        assert_eq!(
            summarize(report).as_deref(),
            Some("last: 200 runs · 0 extra · 0 missed · p50 0.15 · p99 0.31 ms")
        );
        assert_eq!(summarize("skipped: Xvfb is not installed"), None);
    }

    #[test]
    fn profile_names_become_file_names() {
        assert_eq!(file_name("Sober — green flash"), "Sober - green flash");
        assert_eq!(file_name("a/b"), "a-b");
        assert_eq!(file_name("  "), "glint-profile");
    }
}
