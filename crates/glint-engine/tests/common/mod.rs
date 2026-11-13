// SPDX-License-Identifier: MIT
//! What the integration tests share.

use std::sync::{Condvar, Mutex};

/// Sandboxes running at once. Each is an Xvfb plus an engine checking 2,000 times a second; a
/// crowded machine adds blind time to every run and can delay a click past a test's patience.
/// (`cargo nextest` runs each test in its own process; `.config/nextest.toml` limits those.)
static RUNNING: Mutex<usize> = Mutex::new(0);
static FREED: Condvar = Condvar::new();

/// A place among the sandboxes allowed to run at once; dropping it frees the place.
pub struct Slot;

impl Slot {
    pub fn take() -> Self {
        let most = std::thread::available_parallelism().map_or(2, |n| (n.get() / 4).clamp(2, 4));
        let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
        while *running >= most {
            running = FREED.wait(running).unwrap_or_else(|e| e.into_inner());
        }
        *running += 1;
        Slot
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        *RUNNING.lock().unwrap_or_else(|e| e.into_inner()) -= 1;
        FREED.notify_one();
    }
}
