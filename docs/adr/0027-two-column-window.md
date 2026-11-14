# 0027. The window shows its cards in two columns, and its height follows them

- **Status:** Accepted (owner request)
- **Date:** 2026-11-14
- **Plan sections:** §12.2, §12.3, §12.4, §12.8

## Context

The plan made Glint's window like Pulse's: one column of cards, 400 px wide. Glint has more settings than
Pulse: seven cards with *More options* open, about 1,900 px of them, so the window scrolled. The owner asked
for a horizontal layout, without changing the style or anything else. Shown a three-column version (1200 px)
and a two-column one (780 px), they chose two columns, and asked that the window not take much space and that
the Start button not be stretched long.

## Decision

- **Two cards to a row**, each row as tall as its taller card:
  - the hero (Start) beside GAME WINDOW: both say what Start will act on;
  - COLOR beside CLICKING;
  - *More options*: COLOR RANGE under COLOR beside DETECTION under CLICKING, then ENGINE.

  Only `ui/app.slint` changes; the cards, their rows and the theme stay as they were.
- **Width:** 780 px by default and at least 760 px, so each card keeps at least the 360 px Pulse's cards were
  made for (Pulse's are 368 px).
- **Height follows the cards.** The window computes the height that shows them all without scrolling, and
  resizes to it when that changes: when it's first laid out, when *More options* opens or closes, and when a
  warning appears. The height stops at 960 px, which fits a 1080 px screen with a panel and a title bar;
  past that, the cards scroll. A maximized or full-screen window keeps its size.
  - Slint applies a window's preferred size only when it first shows it, so a `changed fit-height`
    handler calls the Rust side, which calls `Window::set_size`.
  - Before the window manager gives the window its first size, Slint reports its minimum size. Resizing to
    that would pin the window at its minimum width, so the handler waits until the window is larger than
    its minimum.

## Consequences

- With *More options* closed, the window is 780 × 542 px and needs no scrolling; before, it was 400 px wide
  and about 800 px of cards.
- With *More options* open, the window is 960 px tall and ENGINE needs a short scroll.
- Tab still follows the cards in reading order, row by row.
- Checked on Xvfb with Openbox: opening at either height, opening and closing *More options*, widening the
  window (the height shrinks as text needs fewer lines), maximizing, and the close dialog.
