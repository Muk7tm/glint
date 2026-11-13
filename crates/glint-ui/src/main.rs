// SPDX-License-Identifier: GPL-3.0-or-later
//! `glint`: the Glint window (PLAN §12). The engine does all the watching and clicking; this
//! window only shows and changes its state over the socket, and never injects input.
#![forbid(unsafe_code)]

mod app;
mod bind;
mod client;
mod keys;
mod spawn;

use glint_ui_view::{Actions, AppWindow, Tone};
use slint::ComponentHandle;

use crate::app::App;
use crate::client::{FromEngine, Link};

fn main() -> Result<(), slint::PlatformError> {
    // Keep the UI off the GPU (§12.8). The app id needs the platform to exist and must be set
    // before the window is shown.
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("software".into())
        .select()?;
    slint::set_xdg_app_id("glint")?;

    let ui = AppWindow::new()?;
    let socket = glint_x11::env::LocalDisplay::from_env().map(|d| glint_ipc::socket_path(d.number));
    let link = match &socket {
        Ok(path) => Link::start(path.clone(), true, app::deliver),
        // No display to watch: the link has nothing to reach, and says why.
        Err(e) => {
            let link = Link::start(std::path::PathBuf::new(), false, |_| {});
            app::deliver(FromEngine::Disconnected(format!("No X display: {e}")));
            link
        }
    };
    App::install(&ui, link);

    let actions = ui.global::<Actions>();
    actions.on_main_button(|| app::with(App::main_button));
    actions.on_form_changed(|| app::with(App::form_changed));
    actions.on_key_captured(|text| app::with(|a| a.key_captured(&text)));
    actions.on_key_chosen(|name| app::with(|a| a.key_chosen(&name)));
    actions.on_hotkey_captured(|text| app::with(|a| a.hotkey_captured(&text)));
    actions.on_hotkey_chosen(|name| app::with(|a| a.hotkey_chosen(&name)));
    actions.on_pick_color(|| app::with(App::pick_color));
    actions.on_cancel_pick(|| app::with(App::cancel_pick));
    actions.on_hex_entered(|text| app::with(|a| a.hex_entered(&text)));
    actions.on_pick_window(|| {
        app::with(|a| {
            a.toast(
                "Picking a window comes with window mode, in a later version.",
                Tone::Neutral,
            )
        });
    });

    // The window manager's close button and Quit Glint follow the close policy (§12.5).
    ui.window().on_close_requested(|| {
        app::with(App::close_requested);
        slint::CloseRequestResponse::KeepWindowShown
    });
    actions.on_close_window(|| app::with(App::close_requested));
    ui.on_keep_running(|| app::with(App::keep_running));
    ui.on_quit_app(|| app::with(App::quit));
    ui.run()
}
