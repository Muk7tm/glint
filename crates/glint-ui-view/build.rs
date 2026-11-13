// SPDX-License-Identifier: GPL-3.0-or-later
//! Compiles the Glint window. The widget style is Pulse's, so the few std widgets the copied
//! components use (scroll views, list views) look the same as in Pulse (ADR 0012).

fn main() {
    let config = slint_build::CompilerConfiguration::new().with_style("cosmic-dark".into());
    if let Err(e) = slint_build::compile_with_config("ui/app.slint", config) {
        panic!("Slint compilation failed: {e}");
    }
}
