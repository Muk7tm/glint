# 0021. Phase 7: v1.0 is checked on the owner's machine, nothing is published, and `install.sh` installs it

- **Status:** Accepted (owner decisions)
- **Date:** 2026-11-13
- **Plan sections:** §12.4, §13, §17, §18, §19

## Context

Phase 7 (§19) packages Glint and checks the v1.0 definition of done. The plan assumed:
- a hardware matrix of other GPUs, desktops and displays (§17), and "at least two GPU vendors" in the definition
  of done;
- published packages: AUR `glint` and `glint-bin`, the second built from release binaries (§18).

At the start of Phase 7 the owner decided:
- no testing on other graphics cards and no publishing of any kind (no AUR uploads, no releases). They want
  neither, and asked that neither be raised again;
- an `install.sh` like Pulse's, which installs Glint and adds it to the apps menu.

Building §19's second item also showed:
- **The window picks its renderer itself.** `glint-ui/src/main.rs` selects winit with the software renderer
  (§12.8), so the menu entry needs no `env SLINT_BACKEND=winit-software`.
- **`Exec=glintd` in the autostart entry needs the engine on the session's `PATH`.** `~/.local/bin` often
  isn't on it when autostart runs.
- **systemd's `Restart=on-failure` would retry exit code 3**, which means another engine already runs on the
  display. It would fail again, over and over.

## Decision

- **v1.0 is judged on the owner's machine** (NVIDIA proprietary driver, Openbox), with no compositor and with
  picom's xrender and glx backends. Every other point of the definition of done stays.
- **Nothing is published.**
  - Packages are built and checked locally.
  - There is no `glint-bin`, because it needs release binaries.
  - `packaging/aur/PKGBUILD` builds from source, for `makepkg -si`.
- **`install.sh`** at the repository root, modelled on Pulse's:
  - **Installs:**
    - builds `glint`, `glintd`, `glint-ctl` and `glint-lab` with `cargo build --release --locked`;
    - copies them into `~/.local/bin` (or `$PREFIX/bin`), then renames each over the old file, so a running
      Glint is safe;
    - installs the menu entry with `Exec` set to the window's full path, and the icon into
      `share/icons/hicolor/scalable/apps`;
    - when *Start the engine when I log in* is on, points its entry at the installed engine.
  - **Offers to restart a running Glint on this display.** It quits the engine and closes that display's
    windows that this install or source folder started. Then it reopens the window, or only the engine if no
    window was open. Glint on other displays is left alone.
  - **`--uninstall`:** stops Glint, removes the files and the autostart entry, and keeps `~/.config/glint`.
  - **Refuses** to run as root. It also refuses an install folder whose path holds `"`, `` ` ``, `$`, `\` or
    `%`, which an `Exec` line can't carry without escaping.
- **Start at login** is a switch in More options → ENGINE: *Start the engine when I log in*.
  - When on, the window writes `~/.config/autostart/glintd.desktop` from `packaging/autostart/glintd.desktop`.
    `Exec` is the engine's full path: the same engine the window starts (§4.3), quoted as the Desktop Entry
    Specification requires.
  - When off, the window deletes the file.
  - The file's presence is the setting, so the switch also works without an engine and isn't in
    `settings.toml`.
- **The systemd unit** adds `RestartPreventExitStatus=3`.
- **The menu entry** (`assets/glint.desktop`):
  - `Exec=glint` and `Icon=glint`;
  - `StartupWMClass=glint`, the window's class;
  - `GenericName=Color Clicker` and search keywords.

## Consequences

- Other drivers, desktops and displays are untested.
- Packages exist only where someone builds them from this repository.
- The owner installs with `./install.sh` and updates by running it again.
- The window shows the same icon as the menu entry. Slint renders `Window.icon` at 64×64 for
  `_NET_WM_ICON`.
