// SPDX-License-Identifier: MIT
//! `glint`: the Glint window (PLAN §12). The engine does all the watching and clicking; this
//! window only shows and changes its state over the socket, and never injects input.
#![forbid(unsafe_code)]

mod app;
mod autostart;
mod bind;
mod client;
mod keys;
mod portal;
mod spawn;

use glint_ui_view::{Actions, AppWindow, Ui};
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
    ui.global::<Ui>().set_version(env!("CARGO_PKG_VERSION").into());
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
    actions.on_check_hex(|text| app::with(|a| a.check_hex(&text)));
    actions.on_add_also(|| app::with(App::add_also));
    actions.on_add_never(|| app::with(App::add_never));
    actions.on_remove_also(|i| app::with(|a| a.remove_also(i)));
    actions.on_remove_never(|i| app::with(|a| a.remove_never(i)));
    actions.on_record(|| app::with(App::record));
    actions.on_choose_recorded(|i| app::with(|a| a.choose_recorded(i)));
    actions.on_sample_for_check(|| app::with(App::sample_for_check));
    actions.on_more_toggled(|| app::with(App::more_toggled));
    actions.on_start_at_login_toggled(|| app::with(App::start_at_login_toggled));
    actions.on_latency_test(|| app::with(App::latency_test));
    actions.on_use_profile(|i| app::with(|a| a.use_profile(i)));
    actions.on_copy_diagnostics(|| app::with(App::copy_diagnostics));
    actions.on_import_profile(|| app::with(App::import_profile));
    actions.on_export_profile(|| app::with(App::export_profile));
    actions.on_key_captured(|text| app::with(|a| a.key_captured(&text)));
    actions.on_key_chosen(|name| app::with(|a| a.key_chosen(&name)));
    actions.on_hotkey_captured(|text| app::with(|a| a.hotkey_captured(&text)));
    actions.on_hotkey_chosen(|name| app::with(|a| a.hotkey_chosen(&name)));
    actions.on_pick_color(|| app::with(App::pick_color));
    actions.on_cancel_pick(|| app::with(App::cancel_pick));
    actions.on_hex_entered(|text| app::with(|a| a.hex_entered(&text)));
    actions.on_pick_window(|| app::with(App::pick_window));

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
