// SPDX-License-Identifier: MIT
//! The window's connection to the engine (PLAN §10). Two threads keep the window from ever waiting
//! on the engine: one blocks reading the socket and hands every message to the window's event
//! loop, the other writes the window's requests. Neither wakes up while nothing happens.
//!
//! At startup, if no engine answers, the link starts one and waits for it (§4.3). After the engine
//! goes away, the link reports it and reconnects only when asked ([`Link::restart`]).

use std::io::{BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use glint_ipc::{ClientKind, Envelope, Event, PROTOCOL, Request};

/// How long a freshly started engine may take to open its socket.
const START_WAIT: Duration = Duration::from_secs(5);

/// What the link tells the window.
#[derive(Clone, PartialEq, Debug)]
pub enum FromEngine {
    /// The engine answered the handshake.
    Connected {
        engine_version: String,
        /// The display is XWayland: only X11 apps can be watched and clicked (§8).
        xwayland: bool,
    },
    /// One message from the engine.
    Event(Box<Event>),
    /// No engine: it couldn't be reached or started, or it went away.
    Disconnected(String),
}

enum Cmd {
    Send(Request),
    /// Send this, then say it was written (or couldn't be).
    Final(Request, Sender<bool>),
    /// Start the engine if needed and connect again.
    Restart,
    /// The reader saw the connection close.
    Closed,
}

/// The window's handle on the link thread.
#[derive(Clone)]
pub struct Link {
    cmds: Sender<Cmd>,
}

impl Link {
    /// Starts the link for the engine socket at `socket`. `deliver` runs on the link's threads,
    /// and must hand the message over to the window's event loop. With `may_start` false the link
    /// never starts an engine itself (tests).
    pub fn start(socket: PathBuf, may_start: bool, deliver: impl Fn(FromEngine) + Send + Clone + 'static) -> Self {
        let (cmds, rx) = mpsc::channel();
        let link = Self { cmds: cmds.clone() };
        let spawned = std::thread::Builder::new()
            .name("glint-link".into())
            .spawn(move || run(socket, may_start, &cmds, &rx, &deliver));
        if let Err(e) = spawned {
            eprintln!("glint: can't start the engine link: {e}");
        }
        link
    }

    pub fn send(&self, request: Request) {
        let _ = self.cmds.send(Cmd::Send(request));
    }

    /// Sends `request` and waits until it has been written, at most `wait`: for the last request
    /// before the window's process exits. False if it couldn't be sent.
    pub fn finish(&self, request: Request, wait: Duration) -> bool {
        let (done, written) = mpsc::channel();
        self.cmds.send(Cmd::Final(request, done)).is_ok() && written.recv_timeout(wait).unwrap_or(false)
    }

    /// Starts the engine if none runs, and connects again.
    pub fn restart(&self) {
        let _ = self.cmds.send(Cmd::Restart);
    }
}

fn run(
    socket: PathBuf,
    may_start: bool,
    cmds: &Sender<Cmd>,
    rx: &Receiver<Cmd>,
    deliver: &(impl Fn(FromEngine) + Send + Clone + 'static),
) {
    let mut start_engine = may_start;
    loop {
        match connect(&socket, start_engine) {
            Ok(stream) => session(stream, cmds, rx, deliver),
            Err(why) => deliver(FromEngine::Disconnected(why)),
        }
        // Wait to be asked again; requests meanwhile can't reach anyone.
        loop {
            match rx.recv() {
                Ok(Cmd::Restart) => break,
                Ok(Cmd::Final(_, done)) => {
                    let _ = done.send(false);
                }
                Ok(Cmd::Send(_) | Cmd::Closed) => {}
                Err(_) => return,
            }
        }
        start_engine = may_start;
    }
}

/// Connects, starting the engine first if `start_engine` and none answers.
fn connect(socket: &PathBuf, start_engine: bool) -> Result<UnixStream, String> {
    if let Ok(s) = UnixStream::connect(socket) {
        return Ok(s);
    }
    if !start_engine {
        return Err("Glint's engine isn't running.".into());
    }
    let mut engine = crate::spawn::start_engine().map_err(|e| {
        format!(
            "Couldn't start Glint's engine ({}): {e}",
            crate::spawn::engine_path().display()
        )
    })?;
    let deadline = Instant::now() + START_WAIT;
    while Instant::now() < deadline {
        if let Ok(s) = UnixStream::connect(socket) {
            crate::spawn::reap(engine);
            return Ok(s);
        }
        // An engine that exits at once won't answer: say so now, not after the wait.
        if let Ok(Some(status)) = engine.try_wait() {
            // Exit code 3: another engine holds the display (§4.3); it may have just started.
            if status.code() == Some(3)
                && let Ok(s) = UnixStream::connect(socket)
            {
                return Ok(s);
            }
            return Err(format!("Glint's engine stopped right away ({status})."));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    crate::spawn::reap(engine);
    Err("Glint's engine didn't start.".into())
}

/// One connection: a reader thread delivers what the engine sends, and this thread writes.
fn session(
    stream: UnixStream,
    cmds: &Sender<Cmd>,
    rx: &Receiver<Cmd>,
    deliver: &(impl Fn(FromEngine) + Send + Clone + 'static),
) {
    let reading = match stream.try_clone() {
        Ok(s) => s,
        Err(e) => return deliver(FromEngine::Disconnected(format!("Can't read from the engine: {e}"))),
    };
    let (closed, out) = (cmds.clone(), deliver.clone());
    let reader = std::thread::Builder::new()
        .name("glint-link-read".into())
        .spawn(move || read(reading, &out, &closed));
    if reader.is_err() {
        return deliver(FromEngine::Disconnected("Can't read from the engine.".into()));
    }
    let mut writer = stream;
    let mut next_id = 1u64;
    let mut write = |writer: &mut UnixStream, request: Request| {
        let line = glint_ipc::encode(&Envelope::new(Some(next_id), request));
        next_id += 1;
        writer.write_all(line.as_bytes())
    };
    let hello = Request::Hello {
        protocol: PROTOCOL,
        client: ClientKind::Ui,
        version: env!("CARGO_PKG_VERSION").to_owned(),
    };
    if write(&mut writer, hello).is_err() {
        return;
    }
    while let Ok(cmd) = rx.recv() {
        match cmd {
            Cmd::Send(request) => {
                if write(&mut writer, request).is_err() {
                    // The reader sees the close too and reports it.
                    let _ = writer.shutdown(std::net::Shutdown::Both);
                }
            }
            Cmd::Final(request, done) => {
                let _ = done.send(write(&mut writer, request).and_then(|()| writer.flush()).is_ok());
            }
            Cmd::Restart => {}
            Cmd::Closed => {
                deliver(FromEngine::Disconnected("Glint's engine stopped.".into()));
                return;
            }
        }
    }
}

fn read(stream: UnixStream, deliver: &impl Fn(FromEngine), closed: &Sender<Cmd>) {
    let mut reader = BufReader::new(stream);
    while let Ok(Some(line)) = glint_ipc::read_line(&mut reader) {
        let Ok(envelope) = serde_json::from_str::<Envelope<Event>>(&line) else {
            continue;
        };
        match envelope.body {
            Event::HelloAck {
                engine_version,
                session,
                ..
            } => deliver(FromEngine::Connected {
                engine_version,
                xwayland: session.xwayland,
            }),
            event => deliver(FromEngine::Event(Box::new(event))),
        }
    }
    let _ = closed.send(Cmd::Closed);
}

#[cfg(test)]
mod tests {
    use std::io::BufRead;
    use std::os::unix::net::UnixListener;

    use glint_ipc::StateSnapshot;

    use super::*;

    /// A stand-in engine: answers hello, echoes a state for every get_state, and lets the test
    /// close it.
    fn fake_engine(socket: &PathBuf) -> std::thread::JoinHandle<Vec<Request>> {
        let listener = UnixListener::bind(socket).unwrap();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut out = stream.try_clone().unwrap();
            let mut seen = Vec::new();
            for line in BufReader::new(stream).lines() {
                let request = serde_json::from_str::<Envelope<Request>>(&line.unwrap()).unwrap().body;
                let reply = match &request {
                    Request::Hello { .. } => Some(Event::HelloAck {
                        protocol: PROTOCOL,
                        engine_version: "9.9".into(),
                        caps: Default::default(),
                        session: Default::default(),
                    }),
                    Request::GetState => Some(Event::State(StateSnapshot::default())),
                    _ => None,
                };
                let quit = request == Request::Quit;
                seen.push(request);
                if let Some(e) = reply {
                    out.write_all(glint_ipc::encode(&Envelope::new(None, e)).as_bytes())
                        .unwrap();
                }
                if quit {
                    break;
                }
            }
            seen
        })
    }

    #[test]
    fn connects_talks_and_reports_the_engine_leaving() {
        let dir = std::env::temp_dir().join(format!("glint-ui-link-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let socket = dir.join("glintd.sock");
        let _ = std::fs::remove_file(&socket);
        let engine = fake_engine(&socket);
        let (tx, rx) = mpsc::channel();
        let link = Link::start(socket.clone(), false, move |m| {
            let _ = tx.send(m);
        });
        let next = || rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(
            next(),
            FromEngine::Connected {
                engine_version: "9.9".into(),
                xwayland: false,
            }
        );
        link.send(Request::GetState);
        assert_eq!(
            next(),
            FromEngine::Event(Box::new(Event::State(StateSnapshot::default())))
        );
        assert!(link.finish(Request::Quit, Duration::from_secs(2)));
        assert!(matches!(next(), FromEngine::Disconnected(_)));
        let seen = engine.join().unwrap();
        assert!(matches!(
            seen[0],
            Request::Hello {
                client: ClientKind::Ui,
                ..
            }
        ));
        assert_eq!(&seen[1..], [Request::GetState, Request::Quit]);
        // With no engine and no permission to start one, it says so.
        let _ = std::fs::remove_file(&socket);
        link.restart();
        assert_eq!(next(), FromEngine::Disconnected("Glint's engine isn't running.".into()));
        // Nothing to write to: a final request says so instead of hanging.
        assert!(!link.finish(Request::Detach, Duration::from_secs(2)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
