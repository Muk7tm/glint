#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
# Build Glint from source and install it for the current user, with an entry in the apps menu.
# No root needed: everything goes into your home folder.
#
#   ./install.sh              build and install (run it again to update)
#   ./install.sh --uninstall  remove Glint (your profiles and settings are kept)
#
# Installs glint, glintd, glint-ctl and glint-lab into ~/.local/bin, glint.desktop into
# ~/.local/share/applications and the icon into ~/.local/share/icons. Set PREFIX to use another
# location (default: ~/.local).
set -euo pipefail

PREFIX="${PREFIX:-$HOME/.local}"
BIN_DIR="$PREFIX/bin"
APP_DIR="$PREFIX/share/applications"
ICON_DIR="$PREFIX/share/icons/hicolor/scalable/apps"
DESKTOP_FILE="$APP_DIR/glint.desktop"
ICON_FILE="$ICON_DIR/glint.svg"
BINARIES=(glint glintd glint-ctl glint-lab)
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BUILT="$ROOT/target/release"

say() { printf '%s\n' "$*"; }
die() {
    printf 'Error: %s\n' "$*" >&2
    exit 1
}
have() { command -v "$1" > /dev/null 2>&1; }

# Yes/no question, default yes. Without a terminal the answer is no.
ask() {
    local reply
    [[ -t 0 ]] || return 1
    read -r -p "$1 [Y/n] " reply || return 1
    [[ -z "$reply" || "$reply" == [Yy]* ]]
}

usage() {
    sed -n '3,11p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
}

# Start at login (More options → Engine) keeps its entry here; an empty or relative
# XDG_CONFIG_HOME counts as unset, as the window reads it.
autostart_file() {
    local config="$HOME/.config"
    [[ "${XDG_CONFIG_HOME:-}" == /* ]] && config="$XDG_CONFIG_HOME"
    printf '%s' "$config/autostart/glintd.desktop"
}

# Whether a Glint engine answers for this display.
engine_running() { [[ -x "$1" ]] && "$1" status > /dev/null 2>&1; }

# The number of the display $1 names: ":0.0" and "localhost:0" are both 0.
display_number() {
    local d="${1#*:}"
    printf '%s' "${d%%.*}"
}

# The Glint windows on this display that this install or this source folder started, by process
# id. Windows on other displays have engines of their own and are left alone.
glint_windows() {
    local pid exe shown here
    here="$(display_number "${DISPLAY:-}")"
    for pid in $(pgrep -u "$UID" -x glint 2> /dev/null); do
        exe="$(readlink "/proc/$pid/exe" 2> /dev/null)" || continue
        exe="${exe% (deleted)}"
        [[ "$exe" == "$BIN_DIR/glint" || "$exe" == "$BUILT/glint" ]] || continue
        shown="$(tr '\0' '\n' < "/proc/$pid/environ" 2> /dev/null | sed -n 's/^DISPLAY=//p')" || continue
        [[ -n "$shown" && "$(display_number "$shown")" == "$here" ]] && printf '%s\n' "$pid"
    done
    return 0
}

# Stops the engine, then closes the windows, which would otherwise offer to restart the old one.
stop_glint() {
    local cli="$1" windows
    windows="$(glint_windows)"
    "$cli" quit > /dev/null 2>&1 || true
    for _ in $(seq 50); do
        engine_running "$cli" || break
        sleep 0.1
    done
    if [[ -n "$windows" ]]; then
        # shellcheck disable=SC2086 # one process id per word
        kill $windows 2> /dev/null || true
    fi
}

# A path as one argument of a desktop entry's Exec key: quoted when it holds a space or another
# character the specification reserves. " ` $ \ and % would need escaping, so such folders are
# refused instead.
desktop_exec() {
    case "$1" in
        *[\"\`\$\\%]*) die "the path $1 has one of \" \` \$ \\ %; choose another folder with PREFIX=…" ;;
        *[[:space:]\'\<\>\~\|\&\;\*\?\#\(\)]*) printf '"%s"' "$1" ;;
        *) printf '%s' "$1" ;;
    esac
}

# Copies the desktop entry $1 to $2 with Exec=$3, leaving out comment lines.
write_entry() {
    awk -v exec="Exec=$3" '/^#/ { next } /^Exec=/ { print exec; next } { print }' "$1" > "$2.new"
    chmod 644 "$2.new"
    mv -f "$2.new" "$2"
}

uninstall() {
    local cli="$BIN_DIR/glint-ctl" autostart
    autostart="$(autostart_file)"
    if engine_running "$cli"; then
        say "Stopping Glint…"
        stop_glint "$cli"
    fi
    local removed=0
    for f in "${BINARIES[@]/#/$BIN_DIR/}" "$DESKTOP_FILE" "$ICON_FILE"; do
        if [[ -e "$f" ]]; then
            rm -f "$f"
            removed=1
        fi
    done
    if [[ -f "$autostart" ]]; then
        rm -f "$autostart"
        say "Glint no longer starts at login."
        removed=1
    fi
    if [[ $removed -eq 0 ]]; then
        say "Glint is not installed in $PREFIX."
        return
    fi
    say "Glint was removed."
    say "Your profiles and settings (~/.config/glint) were kept; delete that folder to remove them too."
}

check_build_tools() {
    # rustup installs cargo here without always adding it to PATH.
    [[ -x "$HOME/.cargo/bin/cargo" ]] && PATH="$HOME/.cargo/bin:$PATH"
    local missing=()
    have cargo || missing+=(rust)
    have cc || missing+=(gcc)
    if have pkg-config; then
        pkg-config --exists fontconfig || missing+=(fontconfig)
    else
        missing+=(pkgconf fontconfig)
    fi
    if [[ ${#missing[@]} -gt 0 ]]; then
        say "Glint needs these to build: ${missing[*]}"
        if have pacman && ask "Install them now with sudo pacman (asks for your password)?"; then
            sudo pacman -S --needed "${missing[@]}"
        elif have pacman; then
            die "install them with: sudo pacman -S --needed ${missing[*]}"
        else
            die "install Rust (https://rustup.rs), a C compiler, pkg-config and the fontconfig development files, then run ./install.sh again"
        fi
    fi
    # Glint uses let chains (edition 2024), stable since Rust 1.88.
    local version major minor
    version="$(cargo --version | awk '{print $2}')"
    IFS=. read -r major minor _ <<< "$version"
    if ((major < 1 || (major == 1 && minor < 88))); then
        die "Rust $version is too old; Glint needs 1.88 or newer (sudo pacman -Syu rust, or rustup update)"
    fi
}

install_files() {
    local exec_window exec_engine autostart
    exec_window="$(desktop_exec "$BIN_DIR/glint")"
    exec_engine="$(desktop_exec "$BIN_DIR/glintd")"
    autostart="$(autostart_file)"
    mkdir -p "$BIN_DIR" "$APP_DIR" "$ICON_DIR"
    # Copy, then rename over the old file: safe while Glint is running.
    for b in "${BINARIES[@]}"; do
        install -m 755 "$BUILT/$b" "$BIN_DIR/.$b.new"
        mv -f "$BIN_DIR/.$b.new" "$BIN_DIR/$b"
    done
    install -m 644 "$ROOT/assets/glint.svg" "$ICON_DIR/.glint.svg.new"
    mv -f "$ICON_DIR/.glint.svg.new" "$ICON_FILE"
    # The menu entry starts the installed window by its full path, so it works even when
    # ~/.local/bin is not on PATH.
    write_entry "$ROOT/assets/glint.desktop" "$DESKTOP_FILE" "$exec_window"
    # Start at login, when it's on, now starts the installed engine.
    if [[ -f "$autostart" ]]; then
        write_entry "$autostart" "$autostart" "$exec_engine"
        say "Start at login now starts the installed engine."
    fi
}

# An engine started before this install keeps running the old copy.
offer_restart() {
    local cli="$BIN_DIR/glint-ctl" windows
    engine_running "$cli" || return 0
    if ! ask "Glint is running. Restart it now to use the new version? Watching stops until you start it again."; then
        say "Glint keeps running the old copy until you quit it (Quit Glint) and open it again."
        return
    fi
    windows="$(glint_windows)"
    stop_glint "$cli"
    if [[ -z "${DISPLAY:-}" ]]; then
        say "Glint stopped. Open it again from your apps menu."
    elif [[ -n "$windows" ]]; then
        setsid -f "$BIN_DIR/glint" > /dev/null 2>&1 < /dev/null
        say "Glint restarted."
    else
        # Its window was closed (Keep running), so only the engine comes back.
        setsid -f "$BIN_DIR/glintd" > /dev/null 2>&1 < /dev/null
        say "Glint's engine restarted, without its window, as it was."
    fi
}

main() {
    case "${1:-}" in
        "") ;;
        --uninstall)
            uninstall
            return
            ;;
        -h | --help)
            usage
            return
            ;;
        *)
            usage >&2
            exit 64
            ;;
    esac
    [[ $EUID -ne 0 ]] || die "run this as your normal user, without sudo: Glint installs into your home folder"
    [[ -f "$ROOT/Cargo.toml" && -d "$ROOT/crates/glint-engine" ]] || die "run install.sh from the Glint source folder"
    if [[ "${XDG_SESSION_TYPE:-}" == wayland ]]; then
        say "Note: Glint needs an X11 session; it does not work on Wayland."
    fi
    # Refuse an unusable folder before the build, not after it.
    desktop_exec "$BIN_DIR/glint" > /dev/null

    check_build_tools
    say "Building Glint (the first build downloads its dependencies and takes a few minutes)…"
    # The engine apart from the window: built together, the window's winit would switch on x11rb's
    # libxcb loading for the engine too, which the engine must never use (PLAN §14).
    (cd "$ROOT" && cargo build --release --locked -p glint-engine -p glint-ctl -p glint-lab &&
        cargo build --release --locked -p glint-ui) || die "the build failed (see above)"

    install_files
    say "Installed Glint into $BIN_DIR and added it to your apps menu."
    case ":$PATH:" in
        *":$BIN_DIR:"*) ;;
        *) say "Note: $BIN_DIR is not on your PATH; use its full path to run glint-ctl in a terminal." ;;
    esac
    have Xvfb || say "Optional: the latency test (More options → Engine) needs Xvfb (on Arch: sudo pacman -S xorg-server-xvfb)."
    offer_restart
    say "Open Glint from your apps menu. To remove it later: ./install.sh --uninstall"
}

main "$@"
