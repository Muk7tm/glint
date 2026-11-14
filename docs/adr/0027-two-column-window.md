# 0027. The window shows its cards in two columns, and *More options* scrolls

- **Status:** Accepted (owner request)
- **Date:** 2026-11-14
- **Plan sections:** §12.2, §12.3, §12.4, §12.8

## Context

The plan made Glint's window like Pulse's: one column of cards, 400 px wide. Glint has more settings than
Pulse: seven cards with *More options* open, about 1,900 px of them, so the window scrolled. The owner asked
for a horizontal layout, without changing the style or anything else. Shown a three-column version (1200 px)
and a two-column one (780 px), they chose two columns, and asked that the window not take much space and that
the Start button not be stretched long. The first version grew the window to fit *More options* when it
opened (to 960 px); the owner asked for it to scroll instead.

## Decision

- **Two cards to a row**, each row as tall as its taller card:
  - the hero (Start) beside GAME WINDOW: both say what Start will act on;
  - COLOR beside CLICKING;
  - *More options*: COLOR RANGE under COLOR beside DETECTION under CLICKING, then ENGINE.

  Only `ui/app.slint` changes; the cards, their rows and the theme stay as they were.
- **Width:** 780 px by default and at least 760 px, so each card keeps at least the 360 px Pulse's cards were
  made for (Pulse's are 368 px).
- **The height fits the part always shown**: the two rows and the *More options* line. Opening *More
  options* doesn't change it: its cards appear below and the window scrolls to them. The window resizes to
  that height when it changes: when it's first laid out, when a warning appears, or when widening the window
  rewraps text. A maximized or full-screen window keeps its size.
  - Slint applies a window's preferred size only when it first shows it, so a `changed fit-height`
    handler calls the Rust side, which calls `Window::set_size`.
  - The height is the always-shown part's own layout, not a sum of its rows: the *More options* line is
    laid out at 30 px but reports 18, and the sum missed that. Measuring the line's position instead made a
    binding loop (the window's height would depend on a position that depends on it), which Slint warns may
    panic.
  - Before the window manager gives the window its first size, Slint reports its minimum size. Resizing to
    that would pin the window at its minimum width, so the handler waits until the window is larger than
    its minimum.

## Consequences

- The window is 780 × 542 px, open or closed. Closed, nothing scrolls; before, it was 400 px wide and about
  800 px of cards.
- Open, COLOR RANGE, DETECTION and ENGINE are a scroll away.
- Tab still follows the cards in reading order, row by row.
- Checked on Xvfb with Openbox: opening with *More options* closed or open, opening and closing it (the
  height stays), scrolling to ENGINE, widening the window (the height shrinks as text needs fewer lines),
  maximizing, and the close dialog.
