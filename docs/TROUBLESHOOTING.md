# Troubleshooting

Start with `glint-ctl status`. It says:

- whether the engine runs, and on which display;
- whether it's watching;
- how it reads the screen, and why;
- whether it runs real-time;
- and, when Glint can't start, what's missing.

For a bug report, use *More options → Engine → Copy diagnostics*, or `glint-ctl diag`.

## A key or button seems stuck

Press **F8**, the emergency stop: it stops watching and lets go of everything. If the engine isn't running,
`glint-ctl release-all` lets go of every key and button Glint could have held. Glint also lets go of everything
whenever it stops, and once more each time it starts.

## Nothing happens when the color appears

- **Is it watching?** The big button says *Stop* while Glint watches. Watching never starts on its own.
- **Was the color already there when you started?** Then Glint waits for it to leave and come back. To get one
  click at the start instead, turn on *Click if it's already there when I start* in CLICKING. It applies only to
  starts you make, never to Glint's own restarts.
- **Does the color count?** *More options → Color range* shows how Glint counts colors:
  - *Check hex* tells you whether a given color counts.
  - *Record 5 s* collects the colors that actually appear at the spot.
  - When the game draws a slightly different shade, choose a looser *Shades* setting, or add the shade with
    *Also match*.
- **Window mode:** with *Only click in this window*, Glint clicks only while that window is in front. The
  *Game window* card says "In front" or "Not in front".
- **Is the spot right?** *Watch* chooses the center of the screen or of the picked window. *More options →
  Detection → Offset* moves the spot.

## It clicks a little late when the color first shows

Some games show a lighter or darker shade for a few frames before the real color. *Record 5 s* shows those
shades. *Loose+* also takes much lighter and darker versions of the color; *Also match* adds one exact shade.

## "Another engine is already running"

Only one engine runs per display. `glint-ctl status` shows the one that runs, and `glint-ctl quit` stops it. A
second engine exits with code 3. The window connects to whichever engine runs.

## The window says *Restart engine*

The engine stopped, maybe through `glint-ctl quit`. Press *Restart engine*. If it stops again at once, run
`glintd` in a terminal to see why.

## Real-time says "off"

See [TUNING.md](TUNING.md) §2: install rtkit, and check `realtime` in `~/.config/glint/settings.toml`. The Max
speed preset never runs real-time, on purpose. With `realtime = "required"`, Glint refuses to start without it
and says why.

## The hotkeys don't work

Glint hears its hotkeys even while a game holds the whole keyboard.

- **Check the key** shown under *Start/stop key*, and the *Mode*. *Hold to watch* watches only while you hold
  the key or button.
- **The click key can't also be a hotkey.** Glint refuses that combination.
- **In a Wayland session (XWayland),** X11 programs hear keys only while an X11 window has the focus.
- **With *Keep the hotkeys from the game* on,** a hotkey that another program has already taken still works,
  but the game gets it too. Glint warns about it.

## "This is XWayland: only X11 apps can be watched and clicked"

You're in a Wayland session. Glint can watch and click X11 programs there, but not Wayland ones. Log into an
X11 (Xorg) session to use Glint everywhere.

## In window mode, Glint reads the screen instead of the window

Under a compositor, Glint reads the picked window's own image, but only after checking that it matches the
screen. `glint-ctl status` says why it fell back:

- the window wasn't in front for the check, so it couldn't compare, and checks again when it comes in front;
- or its image was blank, or didn't match what the screen shows.

Some programs draw into an inner window that this check can't use, so the screen it is.

## Start at login doesn't start Glint

- **Check the entry.** *More options → Engine → Start the engine when I log in* writes
  `~/.config/autostart/glintd.desktop`, with the engine's full path in it.
- **After moving or reinstalling Glint,** turn the switch off and on again, or run `./install.sh` again, which
  updates the entry.
- **Your desktop's session settings** may have turned the entry off.

## The latency test fails

It runs in a hidden test X server, so it needs Xvfb:

| Distribution | Command |
| --- | --- |
| Arch | `sudo pacman -S xorg-server-xvfb` |
| Debian and Ubuntu | `sudo apt install xvfb` |
| Fedora | `sudo dnf install xorg-x11-server-Xvfb` |

## Messages in the terminal

When the window runs from a terminal, it may print `xkbcommon: ERROR: … Compose … not a valid UTF-8 string`.
That's the keyboard library reading your system's compose table under a non-UTF-8 locale. It doesn't affect
Glint.

## Where Glint keeps things

| What | Where |
| --- | --- |
| Settings | `~/.config/glint/settings.toml` (saved automatically) |
| Profiles | `~/.config/glint/profiles/` |
| The engine's socket | `$XDG_RUNTIME_DIR/glint/glintd-<display>.sock` |
| Start at login | `~/.config/autostart/glintd.desktop` |
| The AppImage's copy of its engine | `~/.local/share/glint/<version>/glintd` |
