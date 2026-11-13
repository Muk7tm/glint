// SPDX-License-Identifier: GPL-3.0-or-later
//! `glint`: the Glint window (PLAN §12). The engine does all the watching and clicking; this
//! window only shows and changes its state over the socket, and never injects input.
#![forbid(unsafe_code)]

mod bind;
mod client;
mod spawn;

use glint_ui_view::AppWindow;
use slint::ComponentHandle;

fn main() -> Result<(), slint::PlatformError> {
    // Keep the UI off the GPU (§12.8). The app id needs the platform to exist and must be set
    // before the window is shown.
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("software".into())
        .select()?;
    slint::set_xdg_app_id("glint")?;

    let ui = AppWindow::new()?;
    // The window manager's close button asks first (§12.5).
    let weak = ui.as_weak();
    ui.window().on_close_requested(move || {
        if let Some(ui) = weak.upgrade() {
            ui.invoke_ask_before_closing();
        }
        slint::CloseRequestResponse::KeepWindowShown
    });
    ui.on_keep_running(|| {
        let _ = slint::quit_event_loop();
    });
    ui.on_quit_app(|| {
        let _ = slint::quit_event_loop();
    });
    ui.run()
}
