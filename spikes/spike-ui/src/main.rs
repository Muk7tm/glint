// SPDX-License-Identifier: MIT
//! Phase 0 spike (PLAN §19): Pulse's components in a 400 px window with the hero
//! card and the close-dialog overlay.
//!
//! What it checks on the way: the Slint library path into Pulse's source,
//! `#![forbid(unsafe_code)]` in the hand-written UI crate (the generated view lives
//! in `spike-ui-view`, because its code needs `allow(unsafe_code)`), the software
//! renderer chosen through `BackendSelector`, the XDG app id, and the window
//! manager's close request opening the dialog instead of closing.
//!
//! Flags: `--started` (hero in the Stop state), `--dialog` (open the close dialog
//! after 200 ms), `--quit-after <ms>`.
#![forbid(unsafe_code)]

use std::time::Duration;

use slint::ComponentHandle;
use spike_ui_view::SpikeWindow;

fn main() -> Result<(), slint::PlatformError> {
    let args: Vec<String> = std::env::args().collect();
    let flag = |name: &str| args.iter().any(|a| a == name);
    let quit_after = args
        .iter()
        .position(|a| a == "--quit-after")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse::<u64>().ok());

    // Keep the UI off the GPU (PLAN §12.8). The app id needs the platform to exist
    // and must be set before the window is shown.
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("software".into())
        .select()?;
    slint::set_xdg_app_id("glint-spike")?;

    let ui = SpikeWindow::new()?;
    ui.set_started(flag("--started"));

    // The window manager's close button opens the dialog instead of closing.
    let weak = ui.as_weak();
    ui.window().on_close_requested(move || {
        eprintln!("spike-ui: close requested by the window manager");
        if let Some(ui) = weak.upgrade() {
            ui.invoke_ask_before_closing();
        }
        slint::CloseRequestResponse::KeepWindowShown
    });
    ui.on_keep_running(|| {
        eprintln!("spike-ui: keep running");
        let _ = slint::quit_event_loop();
    });
    ui.on_quit_app(|| {
        eprintln!("spike-ui: quit");
        let _ = slint::quit_event_loop();
    });

    if flag("--dialog") {
        let weak = ui.as_weak();
        slint::Timer::single_shot(Duration::from_millis(200), move || {
            if let Some(ui) = weak.upgrade() {
                ui.invoke_ask_before_closing();
            }
        });
    }
    if let Some(ms) = quit_after {
        slint::Timer::single_shot(Duration::from_millis(ms), || {
            let _ = slint::quit_event_loop();
        });
    }
    ui.run()
}
