// SPDX-License-Identifier: MIT
//! Compiles `ui/app.slint` with Pulse's `ui/` folder as the `@pulse` library, read
//! in place (nothing is copied out of Pulse). This is the mechanism the shared kit
//! will use (PLAN §12.1). Override the location with `PULSE_UI_DIR`.

use std::collections::HashMap;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=PULSE_UI_DIR");
    let pulse_ui = std::env::var_os("PULSE_UI_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../Pulse/crates/pulse-ui/ui"));
    assert!(
        pulse_ui.join("components.slint").exists(),
        "Pulse's UI folder not found at {}; set PULSE_UI_DIR",
        pulse_ui.display()
    );
    // Same widget style as Pulse's build.rs, so the std widgets Pulse uses look the same.
    let config = slint_build::CompilerConfiguration::new()
        .with_style("cosmic-dark".into())
        .with_library_paths(HashMap::from([("pulse".to_string(), pulse_ui)]));
    if let Err(e) = slint_build::compile_with_config("ui/app.slint", config) {
        panic!("Slint compilation failed: {e}");
    }
}
