// SPDX-License-Identifier: MIT
//! The engine's trace stream (§16): every click with its timing, possible misses, blind gaps
//! and self-stops, collected on a connection of its own for the whole run.

use std::io;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use glint_ipc::{Client, ClientKind, Event, ReadError, Request, Trace};

use crate::{LabError, WAIT, engine_err};

pub struct TraceTap {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<Vec<Trace>>>,
}

impl TraceTap {
    pub fn connect(socket: &Path) -> Result<Self, LabError> {
        let mut client = Client::connect(socket).map_err(engine_err)?;
        client.hello(ClientKind::Lab, WAIT).map_err(engine_err)?;
        let reply = client
            .call(
                Request::Subscribe {
                    telemetry_hz: 0,
                    live_hz: 0,
                    trace: true,
                },
                WAIT,
            )
            .map_err(engine_err)?;
        if !matches!(reply, Event::State(_)) {
            return Err(LabError::Engine(format!("subscribe: {reply:?}")));
        }
        let stop = Arc::new(AtomicBool::new(false));
        let quit = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("lab-trace".into())
            .spawn(move || collect(client, &quit))
            .map_err(engine_err)?;
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }

    /// Everything the engine traced. The engine passes trace entries on every few milliseconds,
    /// so this waits briefly, then pings: once the answer arrives, everything before it has too.
    pub fn finish(mut self) -> Vec<Trace> {
        std::thread::sleep(Duration::from_millis(50));
        self.stop.store(true, Ordering::Release);
        self.thread.take().and_then(|t| t.join().ok()).unwrap_or_default()
    }
}

fn collect(mut client: Client, stop: &AtomicBool) -> Vec<Trace> {
    let mut out = Vec::new();
    let mut ping = None;
    loop {
        if ping.is_none() && stop.load(Ordering::Acquire) {
            match client.send(Request::Ping) {
                Ok(id) => ping = Some(id),
                Err(_) => break,
            }
        }
        match client.next_event(Duration::from_millis(50)) {
            Ok(e) => match e.body {
                Event::Trace(t) => out.push(t),
                Event::Pong if ping.is_some() && e.id == ping => break,
                _ => {}
            },
            Err(ReadError::Io(e)) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {}
            // The engine is gone (killed by the scenario, or quit).
            Err(_) => break,
        }
    }
    out
}

impl Drop for TraceTap {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
