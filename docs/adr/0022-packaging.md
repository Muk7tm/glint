# 0022. Packaging: a static engine built apart from the window, staged files, a Debian 11 build in CI, and the engine copied out of the AppImage

- **Status:** Accepted
- **Date:** 2026-11-13
- **Plan sections:** §13, §14, §18, §19

## Context

§18 lists the packages. Building them showed:

1. **The engine and the window can't share one build target.**
   - The window can't be static: it loads its X11 libraries at run time (`dlopen`), which a static musl
     program can't do.
   - cargo-deb and cargo-generate-rpm map their `target/release/` paths to the one target and profile they
     build. Glint's packages hold programs for two: the static engine and the glibc window.
2. **Cargo unifies features within one build command.**
   - Built in the same command as the window, the engine's `x11rb` gets `allow-unsafe-code` and `dl-libxcb`
     through winit. §14 rules both out for the engine.
   - Built without the window, it has neither (`cargo tree -e features -i x11rb`).
3. **An AppImage is mounted at a new path every time it runs.**
   - *Start at login* needs the engine at a path that still exists at the next login.
   - The window starts the engine as a separate process, which outlives the window after *Keep running*.
   - Once the window has closed, the engine's own program file stays available only by accident. On Xvfb,
     with the type 2 runtime, the engine had inherited the runtime's keep-alive pipe from the window. The
     AppImage stayed mounted until the engine quit, not until the window closed.
4. **The owner's machine has neither zig nor a container tool**, so §18's old-baseline glibc build of the
   window can't be made there.
5. **The release profile keeps line tables** for perf traces, so the static engine is about 25 MB.
6. **`cargo-generate-rpm -p` takes the crate's folder**, not its name: 0.21.1 joins `Cargo.toml` to it in
   `main.rs`, whatever its help says.
7. **AppStream metadata** (§13's `metainfo.xml`) only matters to software centers that list published
   packages.

## Decision

- **`packaging/build.sh`** builds into `dist/`, which git ignores:
  - `glintd`, `glint-ctl` and `glint-lab` with `--target x86_64-unknown-linux-musl --profile dist`, in a
    command without the window (point 2). It checks that none has a `NEEDED` entry.
  - The window with `--profile dist`, in a command of its own. It prints the newest glibc version the window
    needs.
  - All four are staged in `dist/stage/`. The `.deb` and `.rpm` metadata in `crates/glint-ui/Cargo.toml`
    takes them from there (point 1).
  - The packages:
    - the AppImage with appimagetool (`--no-appstream`);
    - the `.deb` with cargo-deb 3.8 (`--no-build`);
    - the `.rpm` with cargo-generate-rpm 0.21 (`--auto-req no`, with the dependencies named).
  - A missing tool skips its package, or with `--require` fails the build.
- **`[profile.dist]`** is the release profile with `debug = false` and `strip = "debuginfo"`. Function names
  stay, for backtraces.
- **CI**, in `.github/workflows/packages.yml`, runs `build.sh --require` in a `debian:11` container, so the
  window needs at most glibc 2.31 (point 4). It then checks:
  - each package's contents;
  - that the window needs no glibc newer than 2.31;
  - that every program in the AppImage prints its version.

  It runs when packaging files change, or when started by hand, and uploads nothing.
- **The AppImage:**
  - `AppRun` opens the window, or runs `glintd`, `glint-ctl` or `glint-lab` when named as the first argument.
  - Inside an AppImage, the window copies the engine to `~/.local/share/glint/<version>/glintd` and starts
    that copy. The copy is rewritten when its size differs, and renamed into place. *Start at login* runs it
    too (point 3).
- **The PKGBUILD and `install.sh`** also build the engine and the window in separate commands (point 2).
- **The dependencies are named, not detected.**
  - Loaded at run time, which no detection sees: libX11, libX11-xcb, libxcb, libXcursor, libXi, libXrandr and
    libxkbcommon-x11.
  - Linked: fontconfig and glibc.
- **No `metainfo.xml`** (point 7).

## Consequences

- A package built on the owner's machine needs that machine's glibc; only CI's build is the portable one.
  The owner installs with `install.sh` anyway.
- The AppImage leaves one engine file in `~/.local/share/glint/` for each version that ran.
- Packaging needs readelf and objdump (binutils) on the build machine.
- Sizes from the first local build:

  | What | Size |
  | --- | --- |
  | AppImage | 11.9 MB |
  | `.deb` | 8.0 MB |
  | `.rpm` | 8.3 MB |
  | static engine | 4.2 MB |
  | `glint-ctl` | 2.0 MB |
  | `glint-lab` | 2.1 MB |
  | the window | 19.4 MB |
