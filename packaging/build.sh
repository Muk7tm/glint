#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
# Builds Glint's packages into dist/ (PLAN §18, ADR 0022). Nothing is uploaded anywhere (ADR 0021).
#
#   packaging/build.sh            build every package this machine has the tools for
#   packaging/build.sh --require  fail instead of skipping a package whose tool is missing (CI)
#
# glintd, glint-ctl and glint-lab are fully static (musl); the window is built against this
# machine's glibc (CI builds it on Debian 11, for glibc 2.31). The AppImage needs appimagetool (or
# APPIMAGETOOL=path), the .deb cargo-deb, and the .rpm cargo-generate-rpm.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIST="$ROOT/dist"
STAGE="$DIST/stage"
MUSL=x86_64-unknown-linux-musl
ENGINE=(glintd glint-ctl glint-lab)
REQUIRE=0

say() { printf '%s\n' "$*"; }
die() {
    printf 'Error: %s\n' "$*" >&2
    exit 1
}
have() { command -v "$1" > /dev/null 2>&1; }

# A package whose tool is missing is skipped, or with --require stops the build.
missing() {
    [[ $REQUIRE -eq 0 ]] || die "the $1 needs $2"
    say "Skipped the $1: $2 isn't installed."
}

build_programs() {
    say "Building glintd, glint-ctl and glint-lab (static, $MUSL)…"
    cargo build --profile dist --locked --target "$MUSL" -p glint-engine -p glint-ctl -p glint-lab
    say "Building the window…"
    cargo build --profile dist --locked -p glint-ui
    mkdir -p "$STAGE"
    for b in "${ENGINE[@]}"; do
        install -m 755 "target/$MUSL/dist/$b" "$STAGE/$b"
        # A shared library would tie the engine to one distribution.
        if readelf -d "$STAGE/$b" | grep -q '(NEEDED)'; then
            die "$b needs shared libraries; it must be static"
        fi
    done
    install -m 755 target/dist/glint "$STAGE/glint"
    local glibc
    glibc="$(objdump -T "$STAGE/glint" | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -n 1 || true)"
    say "The window needs ${glibc:-an unknown glibc version} or newer."
    # The packages ask for glibc 2.31 (Debian 11), which only a build on Debian 11 keeps, as CI's does.
    if [[ -n "$glibc" && "$(printf '%s\n' "$glibc" GLIBC_2.31 | sort -V | tail -n 1)" != GLIBC_2.31 ]]; then
        say "Note: the .deb and .rpm ask for glibc 2.31, but this window needs ${glibc#GLIBC_}, so they only"
        say "work where that is present. CI builds the portable ones, on Debian 11."
    fi
}

appimage() {
    local tool="${APPIMAGETOOL:-appimagetool}"
    have "$tool" || {
        missing AppImage appimagetool
        return
    }
    local app="$DIST/Glint.AppDir"
    mkdir -p "$app/usr/bin" "$app/usr/share/applications" "$app/usr/share/icons/hicolor/scalable/apps" \
        "$app/usr/share/licenses/glint"
    install -m 755 "$STAGE/glint" "${ENGINE[@]/#/$STAGE/}" "$app/usr/bin/"
    install -m 755 packaging/appimage/AppRun "$app/AppRun"
    install -m 644 assets/glint.desktop "$app/glint.desktop"
    install -m 644 assets/glint.desktop "$app/usr/share/applications/glint.desktop"
    install -m 644 assets/glint.svg "$app/glint.svg"
    install -m 644 assets/glint.svg "$app/usr/share/icons/hicolor/scalable/apps/glint.svg"
    ln -s glint.svg "$app/.DirIcon"
    install -m 644 LICENSE "$app/LICENSE"
    install -m 644 LICENSE "$app/usr/share/licenses/glint/LICENSE"
    ARCH=x86_64 "$tool" --no-appstream "$app" "$DIST/Glint-$VERSION-x86_64.AppImage"
    rm -rf "$app"
}

deb() {
    have cargo-deb || {
        missing .deb cargo-deb
        return
    }
    # The metadata is in crates/glint-ui/Cargo.toml; its assets are the staged programs.
    cargo deb -p glint-ui --no-build --no-strip --output "$DIST/"
}

rpm() {
    have cargo-generate-rpm || {
        missing .rpm cargo-generate-rpm
        return
    }
    # -p takes the crate's folder (cargo-generate-rpm 0.21 joins Cargo.toml to it), not its name.
    cargo generate-rpm -p crates/glint-ui --auto-req no --output "$DIST/"
}

main() {
    case "${1:-}" in
        "") ;;
        --require) REQUIRE=1 ;;
        *)
            sed -n '3,10p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//' >&2
            exit 64
            ;;
    esac
    cd "$ROOT"
    VERSION="$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)"
    [[ -n "$VERSION" ]] || die "no version in Cargo.toml"
    have readelf && have objdump || die "readelf and objdump (binutils) are needed"
    rm -rf "$DIST"

    build_programs
    appimage
    deb
    rpm

    say "Done: Glint $VERSION in $DIST"
    (cd "$DIST" && find . -maxdepth 2 -type f -printf '  %-40p %s bytes\n' | sort)
}

main "$@"
