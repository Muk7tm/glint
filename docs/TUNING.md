# Tuning Glint for the lowest latency

Glint is fast without tuning. With Balanced checks and no compositor, it answers 99 in 100 appearances within
0.72 ms (MEASUREMENTS.md §11). The settings below matter most when a game keeps the computer busy. They come in
order of how much they matter.

To see where you stand, run `glint-ctl status`. It names how Glint reads the screen, and why, and whether
the engine runs real-time:

```text
reads: screen (no compositor, so the screen shows each frame as it's drawn)
real-time: rtkit (rr 10) · runs on any core
```

## 1. The compositor

A compositor (picom, KWin, Mutter, Xfwm's compositing, and others) draws every window into a hidden image, then
shows the result at the next screen refresh. Glint's *Screen center* mode reads the screen, so it sees a change
only once the compositor has shown it: up to one refresh late, which is 16.7 ms at 60 Hz.

Glint notices a compositor by itself:

- Under one, many checks arrive late.
- Each frame stays on screen for at least one refresh, so Glint lets checks be most of a refresh apart without
  losing track. Re-arming stays on time.
- The *Watch* row says that screen mode sees changes one frame late.

**The fastest choice is no compositor while you play:**

| Desktop | What to do |
| --- | --- |
| picom | Stop it with `pkill -x picom`. Or set `unredir-if-possible = true;` in its config, so full-screen windows skip compositing. |
| KDE Plasma on X11 | Alt+Shift+F12 turns compositing on and off. |
| GNOME on Xorg | Full-screen windows skip compositing on their own. |
| Xfce | Settings → Window Manager Tweaks → Compositor. |

**In window mode** (*Pick window*), under a compositor, Glint reads the window's own image instead of the
screen ("window-buffer"), which skips the compositor's delay.

- It does this only after a self-check: the window's image must match what the screen shows.
- `glint-ctl status` says whether the self-check passed. If it failed, it says why, and Glint reads the screen
  instead.

## 2. Real-time priority (rtkit)

**Why it matters.** When a game keeps the CPU busy, a normal program can wait a few milliseconds before it gets
to run. Real-time priority cuts that wait to tens of microseconds. Glint asks for it through **rtkit**, the
service desktops with PipeWire or PulseAudio already use for audio.

**Checking it.** `glint-ctl status`, or *More options → Engine → Real-time*, should say `rtkit (rr 10)`. If it
says `off`:

- **Install rtkit, then restart Glint:**
  - Arch: `sudo pacman -S rtkit`
  - Debian and Ubuntu: `sudo apt install rtkit`
  - Fedora: `sudo dnf install rtkit`
- **Check that real-time isn't switched off** in `~/.config/glint/settings.toml`:

  ```toml
  [engine]
  realtime = "auto"   # "off", "auto" (the default), or "required": refuse to start without it
  ```

**The Max speed preset never runs real-time.** Its checking thread never pauses, and a real-time thread that
never pauses would starve everything else.

**If the checking thread ever runs too long without a pause,** Glint drops it to normal priority and warns:
"Real-time priority is off: the engine ran too long without a pause." It stays at normal priority until the
engine restarts.

## 3. The CPU governor

Power-saving CPU governors let the processor slow down and sleep between checks, and waking it costs time.

- **See the current governor:** `cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor`.
- **Switch to `performance` until the next reboot:** either of these works.
  - With power-profiles-daemon: `powerprofilesctl set performance`
  - With cpupower: `sudo cpupower frequency-set -g performance`

## 4. The speed preset

*More options → Detection → Checks*:

| Preset | Checks per second | When to use it |
| --- | --- | --- |
| Eco | 500 | Laptops on battery. It waits 1 ms per check on average, and loads the X server less. |
| Balanced | 2,000 | The default. It waits 0.25 ms on average. |
| Max | As many as possible | Rarely worth it. It makes other programs' requests to the X server slower and gains almost nothing (MEASUREMENTS.md §3). |

## 5. Rarely needed

- **Pinning to a CPU core:** in `settings.toml`, set `[engine] cpu` to a core number such as `3`. The default,
  `"none"`, lets the kernel choose; `"auto"` doesn't pin either, because measurements showed no benefit.
- **Locking memory:** Glint locks its memory only when the memory-lock limit is unlimited. A finite limit could
  make a later allocation fail. Its checking loop never allocates anyway.

  To allow it, create `/etc/security/limits.d/glint.conf` containing the line below, with your own user name.
  Then log out and in again.

  ```text
  youruser - memlock unlimited
  ```

## Measuring

- **The quick test:** *More options → Engine → Latency test* shows and clicks 200 appearances in a hidden test
  X server. It needs Xvfb.
- **The real test:**
  1. Quit Glint first, because only one engine runs per display.
  2. Run `glint-lab basic --count 1000 --real-desktop`. It shows a full-screen test window and clicks it.
     Keep your hands off the mouse and keyboard; F8 stops it.
  3. Read the report:
     - **response:** how fast Glint pressed;
     - **split:** where that time went;
     - **stalls:** pauses of the X server itself, which no client can avoid.
