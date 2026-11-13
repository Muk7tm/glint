// SPDX-License-Identifier: MIT
//! Starting the engine when none answers (§4.3): `glintd` next to this program, else the one on
//! `$PATH`. It runs on its own, not as part of the window, so closing the window with *Keep
//! running* leaves it watching.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

/// The engine program to start, and the one *Start at login* starts.
///
/// Inside an AppImage the engine is first copied out (ADR 0022). An AppImage is mounted at a new
/// path every time it runs, and *Start at login* needs one that lasts. The engine, which outlives
/// the window after *Keep running*, also shouldn't depend on the AppImage staying mounted.
pub fn engine_path() -> PathBuf {
    let Some(beside) = std::env::current_exe()
        .ok()
        .map(|p| p.with_file_name("glintd"))
        .filter(|p| p.exists())
    else {
        return PathBuf::from("glintd");
    };
    match (appimage_dir(), data_home()) {
        (Some(mount), Some(data)) if beside.starts_with(&mount) => copied_out(&beside, &data).unwrap_or(beside),
        _ => beside,
    }
}

/// Where the AppImage this program runs from is mounted, if it runs from one: its runtime sets
/// `APPIMAGE` to the file and `APPDIR` to the mount.
fn appimage_dir() -> Option<PathBuf> {
    std::env::var_os("APPIMAGE")?;
    std::env::var_os("APPDIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

/// `$XDG_DATA_HOME`, else `~/.local/share` (an empty or relative value counts as unset).
fn data_home() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .map(|home| home.join(".local/share"))
        })
}

/// A copy of `engine` that outlives the AppImage: `<data>/glint/<version>/glintd` (still named
/// `glintd`, for `ps` and `pkill`), written when it's missing or differs in size, and renamed into
/// place so a running copy is never touched.
fn copied_out(engine: &Path, data: &Path) -> io::Result<PathBuf> {
    let dir = data.join("glint").join(env!("CARGO_PKG_VERSION"));
    let copy = dir.join("glintd");
    let size = std::fs::metadata(engine)?.len();
    if std::fs::metadata(&copy).map(|m| m.len()).ok() != Some(size) {
        std::fs::create_dir_all(&dir)?;
        let fresh = dir.join(format!(".glintd-{}.new", std::process::id()));
        std::fs::copy(engine, &fresh)?;
        std::fs::rename(&fresh, &copy)?;
    }
    Ok(copy)
}

/// Starts the engine and returns it, so the caller can notice if it exits at once.
pub fn start_engine() -> io::Result<Child> {
    Command::new(engine_path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
}

/// Waits for `child` on a thread of its own, so it never lingers as a zombie while the window is
/// open; if the window closes first, the engine simply keeps running.
pub fn reap(mut child: Child) {
    let _ = std::thread::Builder::new()
        .name("glint-engine-wait".into())
        .spawn(move || {
            let _ = child.wait();
        });
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    #[test]
    fn the_engine_is_copied_out_once_and_again_when_it_changes() {
        let root = std::env::temp_dir().join(format!("glint-spawn-test-{}", std::process::id()));
        let mount = root.join("mount/usr/bin");
        std::fs::create_dir_all(&mount).unwrap();
        let engine = mount.join("glintd");
        std::fs::write(&engine, b"engine v1").unwrap();
        std::fs::set_permissions(&engine, std::fs::Permissions::from_mode(0o755)).unwrap();
        let data = root.join("data");

        let copy = copied_out(&engine, &data).unwrap();
        assert_eq!(copy, data.join(format!("glint/{}/glintd", env!("CARGO_PKG_VERSION"))));
        assert_eq!(std::fs::read(&copy).unwrap(), b"engine v1");
        assert_eq!(std::fs::metadata(&copy).unwrap().permissions().mode() & 0o777, 0o755);
        // The same engine again: the copy stays as it is.
        let modified = std::fs::metadata(&copy).unwrap().modified().unwrap();
        assert_eq!(copied_out(&engine, &data).unwrap(), copy);
        assert_eq!(std::fs::metadata(&copy).unwrap().modified().unwrap(), modified);
        // A different engine of the same version (another build): replaced, nothing left beside it.
        std::fs::write(&engine, b"engine v1, rebuilt").unwrap();
        assert_eq!(copied_out(&engine, &data).unwrap(), copy);
        assert_eq!(std::fs::read(&copy).unwrap(), b"engine v1, rebuilt");
        assert_eq!(std::fs::read_dir(copy.parent().unwrap()).unwrap().count(), 1);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
