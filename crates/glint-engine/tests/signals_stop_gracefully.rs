// SPDX-License-Identifier: GPL-3.0-or-later
//! SIGTERM, SIGINT and SIGHUP reach the graceful-stop handler (§4.3). Signal handlers are
//! process-wide, so this test has a process of its own.

use std::sync::mpsc;
use std::time::Duration;

use glint_engine::lifecycle::watch_signals;
use signal_hook::consts::signal::{SIGHUP, SIGINT, SIGTERM};

#[test]
fn each_stop_signal_reaches_the_handler() {
    let (tx, rx) = mpsc::channel();
    watch_signals(move |s| tx.send(s).unwrap()).unwrap();
    for signal in [SIGHUP, SIGTERM, SIGINT] {
        signal_hook::low_level::raise(signal).unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), signal);
    }
}
