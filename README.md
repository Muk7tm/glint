# Glint

Glint clicks the moment a color appears on your screen. It watches a few pixels at the center of your monitor,
or of one window you pick. When they turn the color you chose, Glint sends one mouse click or key press. Then it
waits until that color has clearly gone and come back before it presses again.

It runs on Linux in an X11 session (Xorg). In its timing run, Glint answered 10,000 appearances with exactly
10,000 clicks (no compositor running):

- 99 in 100 within 0.72 ms;
- none slower than 1.05 ms.

The details are in [docs/MEASUREMENTS.md](docs/MEASUREMENTS.md) §11.

> **Before you use it in a game:** many games' rules, Roblox's among them, treat automated input as cheating
> and can ban accounts for it. Use Glint only where automation is allowed. Glint has no anti-detection or
> "humanized" input, and won't get any.

## Install

From this folder, as your normal user (no `sudo`):

```sh
./install.sh
```

- **What it does:**
  - builds Glint;
  - installs its four programs into `~/.local/bin`;
  - adds Glint to your apps menu with its icon.
- **Updating:** run it again. If Glint is running, it offers to restart it on the new version.
- **Removing:** `./install.sh --uninstall` removes Glint and keeps your profiles and settings in
  `~/.config/glint`.
- **What the build needs:** Rust 1.88 or newer, a C compiler, pkg-config and fontconfig.
  - On Arch: `sudo pacman -S --needed rust gcc pkgconf fontconfig`.
  - When something is missing, the script names it, and on Arch it offers to install it.
- **Optional:**
  - **rtkit**, which gives Glint's engine real-time priority (see [docs/TUNING.md](docs/TUNING.md));
  - **Xvfb**, for the latency test in *More options*.

Other ways to build it:

- **Arch package:** `packaging/aur/PKGBUILD`, installed with `makepkg -si`.
- **AppImage, `.deb` and `.rpm`:** `packaging/build.sh` builds them into `dist/`.

## Use

1. Open **Glint** from your apps menu.
2. **COLOR:**
   - Press *Pick* and click the color on screen, or type its hex code.
   - *Shades* sets how far from it still counts: *Tight*, *Normal*, *Loose* or *Loose+*. *Loose+* also takes
     much lighter and darker versions of the color.
3. **CLICKING:**
   - Choose what Glint presses: a mouse button, including the side buttons, or a key.
   - Choose how you start it: *Press to toggle* (F7 by default), or *Hold to watch*, which watches only while
     you hold a key or mouse button.
4. **GAME WINDOW** (optional): *Pick window* makes Glint watch that window's center. With *Only click in this
   window*, it clicks only while that window is in front.
5. Press **Start**, or your start key. Glint never starts watching on its own.

**F8 stops everything at once** and lets go of any key or button Glint holds. F6 samples the color at the
center.

When you close the window, Glint asks whether to keep running in the background, where your hotkeys still
work, or to quit. *More options* has:

- the detection pattern and speed;
- the exact color range;
- the engine's priority and a latency test;
- profiles, *Start the engine when I log in*, and diagnostics for bug reports.

## One click per appearance

Glint presses on the very first check that sees your color: speed comes first when the color arrives. All the
caution goes into deciding that the color has really left:

- **Re-arming:** Glint presses again only after it has seen the color stay gone for 40 ms (*Gone after*).
- **Starting while the color shows:** when you start Glint while your color is already showing, it waits for
  the color to leave first. Turn on *Click if it's already there when I start* to get one click right away
  instead. That happens only on a start you make, never when Glint restarts on its own.
- **Possible misses:** when the system stalls and Glint can't be sure it saw everything, it reports a
  possible miss instead of guessing.

## Command line

`glint-ctl` controls the engine from a terminal:

| Command | What it does |
| --- | --- |
| `status` | whether the engine runs, and what it's doing |
| `start`, `stop`, `toggle` | start or stop watching |
| `panic` | the emergency stop: stop watching and let go of everything |
| `release-all` | let go of every key and button Glint could hold, even when no engine runs |
| `quit` | stop the engine |
| `profile` | manage profiles |
| `classify '#RRGGBB'` | say how the active profile counts that color |
| `diag` | everything about the engine, for a bug report |

## Documentation

- [docs/TUNING.md](docs/TUNING.md): the lowest latency (compositor, real-time priority, CPU governor).
- [docs/TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md): when something doesn't work.
- [docs/PLAN.md](docs/PLAN.md): the full design.
- [docs/MEASUREMENTS.md](docs/MEASUREMENTS.md): measured numbers.
- [docs/adr/](docs/adr/): the decisions that changed the design.

## Building and testing

```sh
cargo build --workspace
cargo test --workspace     # the integration tests start their own Xvfb
cargo run -p glint-lab -- basic --count 100
```

`glint-lab` runs test scenarios in a hidden X server and judges every click: extra, missed and late. With
`--real-desktop`, it shows a full-screen test window and clicks it. Keep your hands off the mouse and keyboard
while it runs; F8 stops it.

## License

MIT, see [LICENSE](LICENSE). Glint's window uses [Slint](https://slint.dev) under its royalty-free license;
the *About* dialog credits it.
