// SPDX-License-Identifier: MIT
//! The engine's trace stream (§16): every click with its timing, possible misses, blind gaps
//! and self-stops, collected on a connection of its own for the whole run.
//!
//! The engine never waits for a reader: one that falls behind by a full outbox is let go, and
//! nothing after that reaches it. On a busy machine that can happen to the lab, so the tap says
//! whether the stream reached the run's end, and the judge doesn't take a cut-off trace for a
//! whole one. A thread reads the stream without timeouts, so no line is ever cut in two.

use std::io::{BufReader, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::thread::JoinHandle;
use std::time::Duration;

use glint_ipc::{Client, ClientKind, Envelope, Event, Request, Trace, decode, encode, read_line};

use crate::{LabError, WAIT, engine_err};

pub struct TraceTap {
    writer: UnixStream,
    /// The id of the closing ping.
    closing: u64,
    thread: Option<JoinHandle<Traced>>,
}

/// What the tap collected.
#[derive(Debug, Default)]
pub struct Traced {
    pub entries: Vec<Trace>,
    /// The engine closed the connection before the run's end, so entries after some point
    /// never arrived.
    pub lost: bool,
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
        let (mut reader, writer, closing) = client.into_parts();
        reader.get_ref().set_read_timeout(None).map_err(engine_err)?;
        let thread = std::thread::Builder::new()
            .name("lab-trace".into())
            .spawn(move || collect(&mut reader, closing))
            .map_err(engine_err)?;
        Ok(Self {
            writer,
            closing,
            thread: Some(thread),
        })
    }

    /// Everything the engine traced. The engine passes trace entries on every few milliseconds,
    /// so this waits briefly, then pings: once the answer arrives, everything before it has too.
    pub fn finish(mut self) -> Traced {
        std::thread::sleep(Duration::from_millis(50));
        let ping = encode(&Envelope::new(Some(self.closing), Request::Ping));
        if self.writer.write_all(ping.as_bytes()).is_err() {
            // The engine is gone: end the reader's wait.
            let _ = self.writer.shutdown(Shutdown::Both);
        }
        self.thread.take().and_then(|t| t.join().ok()).unwrap_or(Traced {
            entries: Vec::new(),
            lost: true,
        })
    }
}

fn collect(reader: &mut BufReader<UnixStream>, closing: u64) -> Traced {
    let mut entries = Vec::new();
    // Only the answer to the closing ping proves that nothing went missing.
    let lost = loop {
        let line = match read_line(reader) {
            Ok(Some(line)) => line,
            // The engine closed the connection: it let the tap go, or it's gone.
            Ok(None) | Err(_) => break true,
        };
        match decode::<Event>(&line) {
            Ok(e) => match e.body {
                Event::Trace(t) => entries.push(t),
                Event::Pong if e.id == Some(closing) => break false,
                _ => {}
            },
            Err(_) => break true,
        }
    };
    Traced { entries, lost }
}

impl Drop for TraceTap {
    fn drop(&mut self) {
        // A tap dropped without `finish` ends the reader's wait here.
        let _ = self.writer.shutdown(Shutdown::Both);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
