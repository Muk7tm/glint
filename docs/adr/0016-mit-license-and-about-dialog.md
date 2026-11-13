# 0016. MIT license, with Pulse's About dialog showing AboutSlint

- **Status:** Accepted (owner decision)
- **Date:** 2026-11-13
- **Plan sections:** §2, §12.3, §13, §14, §16 (CI), §18, §21
- **Supersedes:** the GPL-3.0-or-later decision of §21 (PLAN v1)

## Context

The plan licensed Glint GPL-3.0-or-later, so that Slint could be used under GPLv3 without any on-screen credit.
The owner asked to switch to MIT and to add an "About" button beside *Quit*, exactly like Pulse's.

Slint 1.18.1 is offered as `GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0`.
The royalty-free license (`slint-1.18.1/LICENSES/LicenseRef-Slint-Royalty-free-2.0.md`) covers desktop
applications. Its condition, §2 "Attribution", is either of:

- (a) the `AboutSlint` widget in an "About" screen or dialog reachable from the top level of the app;
- (b) the Slint attribution badge on a public web page, preferably the download page.

It doesn't allow distributing Slint on its own, use in embedded systems, or exposing Slint's APIs.

## Decision

1. **Glint is MIT.**
   - `LICENSE` holds the MIT text ("Copyright (c) 2026 The Glint authors"; Pulse's says "Pulse Macro").
   - The workspace sets `license = "MIT"`, and every source file has `SPDX-License-Identifier: MIT`.
   - `glintd`, `glint-ctl` and `glint-lab` say MIT in their help and in `glint-ctl diag`.
2. **About, exactly as in Pulse.**
   - A ghost *About* button sits before *Quit Glint*.
   - It opens Pulse's 300 px popup: "Glint" and the version, one line on what Glint is, "License: MIT.",
     `AboutSlint`, and *Close*. Escape or a click outside closes it.
   - The popup is copied from Pulse's `main.slint` (at `e4ccdaa`, MIT), with a note of its origin. Pulse's
     repository is untouched.
   - This meets condition (a).
3. **`deny.toml` allows `LicenseRef-Slint-Royalty-free-2.0` and no GPL license.** `cargo deny check licenses`
   passes without GPL, so no dependency needed it. A future GPL-only dependency now fails CI, as it should
   for an MIT project.

## Consequences

- Anyone may reuse Glint's code under MIT. The `glint` binary stays under Slint's royalty-free terms, which
  hold as long as the About dialog shows `AboutSlint`. Removing it would break them.
- The engine, `glint-ctl` and `glint-lab` don't link Slint.
- The window already set no version: it now sets `Ui.version` from the package version for the dialog.
