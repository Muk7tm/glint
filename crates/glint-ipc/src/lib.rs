// SPDX-License-Identifier: GPL-3.0-or-later
//! Glint's IPC protocol (PLAN §10, Appendix A.5): message types, newline-delimited JSON framing,
//! the socket's location, and a small blocking client.
//!
//! One JSON object per line, at most 1 MiB, with a `"type"` tag and an optional `"id"` that the
//! direct reply echoes. Adding fields or message types keeps [`PROTOCOL`]; breaking changes bump
//! it.
#![forbid(unsafe_code)]

use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use glint_core::{Class, Profile, Rgb, Settings, Verdict};
use serde::{Deserialize, Serialize};

/// The protocol version both sides must agree on.
pub const PROTOCOL: u32 = 1;

/// The longest line either side accepts.
pub const MAX_LINE: usize = 1 << 20;

/// The most colors one `classify` request may ask about.
pub const MAX_CLASSIFY: usize = 256;

/// A message with its optional request id.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Envelope<T> {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    #[serde(flatten)]
    pub body: T,
}

impl<T> Envelope<T> {
    pub fn new(id: Option<u64>, body: T) -> Self {
        Self { id, body }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientKind {
    Ui,
    Ctl,
    Lab,
}

/// Client → engine.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    Hello {
        protocol: u32,
        client: ClientKind,
        version: String,
    },
    GetState,
    /// Telemetry (0–10 per second) and the live swatch (0–20 per second); 0 turns a stream off.
    Subscribe {
        telemetry_hz: u8,
        live_hz: u8,
    },
    SetProfile {
        profile: Box<Profile>,
    },
    /// The named profile, or the active one (an addition to §10, for `glint-ctl profile show`).
    GetProfile {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    UseProfile {
        name: String,
    },
    ListProfiles,
    SetSettings {
        settings: Box<Settings>,
    },
    Start,
    Stop,
    Toggle,
    EmergencyStop,
    ReleaseAll,
    SampleCenter,
    /// Up to [`MAX_CLASSIFY`] colors, classified with the engine's own lookup table.
    Classify {
        colors: Vec<Rgb>,
    },
    /// The client leaves; the engine keeps running.
    Detach,
    /// The engine stops, releases input, removes its socket and exits.
    Quit,
    Ping,
}

/// Engine → client.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    HelloAck {
        protocol: u32,
        engine_version: String,
        caps: CapsInfo,
        session: SessionInfo,
    },
    State(StateSnapshot),
    Telemetry(TelemetrySnapshot),
    Live {
        rgb: Rgb,
        verdict: Verdict,
    },
    Fired {
        count: u64,
        latency_us: u32,
    },
    ColorSampled {
        rgb: Rgb,
        oklch: [f32; 3],
    },
    Classified {
        verdicts: Vec<Class>,
    },
    Profiles {
        names: Vec<String>,
        active: String,
    },
    Profile {
        profile: Box<Profile>,
    },
    ProfileApplied {
        rev: u64,
    },
    SettingsApplied,
    Warning {
        code: String,
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        field: Option<String>,
    },
    Error {
        code: String,
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        field: Option<String>,
    },
    Pong,
    Bye,
}

impl Event {
    pub fn error(code: &str, message: impl Into<String>, field: Option<String>) -> Self {
        Self::Error {
            code: code.to_owned(),
            message: message.into(),
            field,
        }
    }
}

/// The server's extensions, as `hello_ack` reports them.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct CapsInfo {
    pub xtest: String,
    pub shm: bool,
    pub damage: bool,
    pub composite: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub randr: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xi2: Option<String>,
}

#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct SessionInfo {
    pub display: String,
    pub xwayland: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compositor: Option<String>,
    pub realtime: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    #[default]
    Stopped,
    /// Started and checking.
    Watching,
    /// Started, but the target window isn't in front.
    Paused,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    #[default]
    Screen,
    Window,
}

#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct TargetInfo {
    pub kind: TargetKind,
    /// The monitor's or the window's name.
    pub name: String,
}

/// The engine's state (§10 `state`).
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct StateSnapshot {
    pub started: bool,
    pub phase: Phase,
    pub target: TargetInfo,
    pub backend: String,
    /// The target check rate; `None` for Max.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checks_per_sec: Option<u32>,
    pub profile: String,
    pub revision: u64,
    /// Why watching stopped last, in the UI's words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_stop: Option<String>,
    /// Why Start would be refused right now ("Pick a color to start").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cannot_start: Option<String>,
}

/// Live statistics (§10 `telemetry`, §17).
#[derive(Clone, PartialEq, Debug, Default, Serialize, Deserialize)]
pub struct TelemetrySnapshot {
    pub checks_per_sec: f64,
    pub capture_p50_us: f64,
    pub capture_p99_us: f64,
    pub decide_p50_us: f64,
    pub clicks: u64,
    pub suppressed: u64,
    pub maybe_missed: u64,
    pub stalls: u64,
    pub worst_stall_ms: f64,
    pub dropped: u64,
}

/// Where engines put their sockets: `$XDG_RUNTIME_DIR/glint`, or `/tmp/glint-<uid>` without one.
pub fn socket_dir() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()) {
        Some(dir) => Path::new(&dir).join("glint"),
        None => PathBuf::from(format!("/tmp/glint-{}", rustix::process::getuid().as_raw())),
    }
}

/// The engine's socket for display `:N` (§10).
pub fn socket_path(display_number: u16) -> PathBuf {
    socket_dir().join(format!("glintd-{display_number}.sock"))
}

/// One message as a line of JSON.
pub fn encode<T: Serialize>(message: &Envelope<T>) -> String {
    let mut line = serde_json::to_string(message).unwrap_or_default();
    line.push('\n');
    line
}

#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("a line longer than {MAX_LINE} bytes")]
    TooLong,
    #[error("not a valid message: {0}")]
    Json(#[from] serde_json::Error),
}

/// Reads one line of at most [`MAX_LINE`] bytes; `None` at the end of the stream.
pub fn read_line(reader: &mut impl BufRead) -> Result<Option<String>, ReadError> {
    let mut line = Vec::new();
    let n = reader.by_ref().take(MAX_LINE as u64 + 1).read_until(b'\n', &mut line)?;
    if n == 0 {
        return Ok(None);
    }
    if line.last() != Some(&b'\n') && n > MAX_LINE {
        return Err(ReadError::TooLong);
    }
    while line.last().is_some_and(|b| *b == b'\n' || *b == b'\r') {
        line.pop();
    }
    String::from_utf8(line)
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e).into())
}

/// A blocking client.
pub struct Client {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    next_id: u64,
}

impl Client {
    pub fn connect(path: &Path) -> io::Result<Self> {
        let stream = UnixStream::connect(path)?;
        Ok(Self {
            reader: BufReader::new(stream.try_clone()?),
            writer: stream,
            next_id: 1,
        })
    }

    /// Sends a request and returns its id.
    pub fn send(&mut self, request: Request) -> io::Result<u64> {
        let id = self.next_id;
        self.next_id += 1;
        self.writer
            .write_all(encode(&Envelope::new(Some(id), request)).as_bytes())?;
        Ok(id)
    }

    /// The next message from the engine, waiting at most `timeout`.
    pub fn next_event(&mut self, timeout: Duration) -> Result<Envelope<Event>, ReadError> {
        self.reader.get_ref().set_read_timeout(Some(timeout))?;
        match read_line(&mut self.reader)? {
            Some(line) => Ok(serde_json::from_str(&line)?),
            None => Err(io::Error::from(io::ErrorKind::UnexpectedEof).into()),
        }
    }

    /// Sends a request and waits for its reply, skipping unrelated events.
    pub fn call(&mut self, request: Request, timeout: Duration) -> Result<Event, ReadError> {
        let id = self.send(request)?;
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return Err(io::Error::from(io::ErrorKind::TimedOut).into());
            }
            let event = self.next_event(left)?;
            if event.id == Some(id) {
                return Ok(event.body);
            }
        }
    }

    /// The engine's process id, from the socket's peer credentials.
    pub fn peer_pid(&self) -> Option<i32> {
        rustix::net::sockopt::socket_peercred(&self.writer)
            .ok()
            .map(|c| c.pid.as_raw_nonzero().get())
    }

    /// `hello` with the protocol check.
    pub fn hello(&mut self, client: ClientKind, timeout: Duration) -> Result<Event, ReadError> {
        let version = env!("CARGO_PKG_VERSION").to_owned();
        self.call(
            Request::Hello {
                protocol: PROTOCOL,
                client,
                version,
            },
            timeout,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_in_the_plans_shape() {
        let hello: Envelope<Request> =
            serde_json::from_str(r#"{"id":1,"type":"hello","protocol":1,"client":"ui","version":"0.1.0"}"#).unwrap();
        assert_eq!(
            hello,
            Envelope::new(
                Some(1),
                Request::Hello {
                    protocol: 1,
                    client: ClientKind::Ui,
                    version: "0.1.0".into()
                }
            )
        );
        let start: Envelope<Request> = serde_json::from_str(r#"{"type":"start"}"#).unwrap();
        assert_eq!(start, Envelope::new(None, Request::Start));
        assert_eq!(
            encode(&Envelope::new(Some(7), Request::GetState)),
            "{\"id\":7,\"type\":\"get_state\"}\n"
        );
        let classify = encode(&Envelope::new(
            None,
            Request::Classify {
                colors: vec![Rgb::new(0x3B, 0xD1, 0x6F)],
            },
        ));
        assert_eq!(classify, "{\"type\":\"classify\",\"colors\":[\"#3BD16F\"]}\n");
        assert!(serde_json::from_str::<Envelope<Request>>(r#"{"type":"launch_rockets"}"#).is_err());
    }

    #[test]
    fn every_message_round_trips() {
        let requests = [
            Request::Subscribe {
                telemetry_hz: 10,
                live_hz: 20,
            },
            Request::SetProfile {
                profile: Box::new(Profile::new("x")),
            },
            Request::SetSettings {
                settings: Box::default(),
            },
            Request::UseProfile { name: "Sober".into() },
            Request::EmergencyStop,
            Request::Classify {
                colors: vec![Rgb::new(1, 2, 3)],
            },
        ];
        for r in requests {
            let line = encode(&Envelope::new(Some(3), r.clone()));
            assert_eq!(
                serde_json::from_str::<Envelope<Request>>(&line).unwrap().body,
                r,
                "{line}"
            );
        }
        let events = [
            Event::State(StateSnapshot {
                started: true,
                phase: Phase::Watching,
                ..StateSnapshot::default()
            }),
            Event::Telemetry(TelemetrySnapshot {
                clicks: 3,
                ..TelemetrySnapshot::default()
            }),
            Event::Live {
                rgb: Rgb::new(1, 2, 3),
                verdict: Verdict::Enter,
            },
            Event::Classified {
                verdicts: vec![Class::Tight, Class::Excluded],
            },
            Event::error("invalid", "Must be between 1 and 9.", Some("sampling.k_enter".into())),
            Event::Bye,
        ];
        for e in events {
            let line = encode(&Envelope::new(None, e.clone()));
            assert_eq!(
                serde_json::from_str::<Envelope<Event>>(&line).unwrap().body,
                e,
                "{line}"
            );
        }
    }

    #[test]
    fn line_limits() {
        let mut ok = io::Cursor::new(b"{\"type\":\"ping\"}\r\n{\"type\":\"pong\"}".to_vec());
        assert_eq!(read_line(&mut ok).unwrap().as_deref(), Some("{\"type\":\"ping\"}"));
        assert_eq!(read_line(&mut ok).unwrap().as_deref(), Some("{\"type\":\"pong\"}"));
        assert_eq!(read_line(&mut ok).unwrap(), None);
        let mut long = io::Cursor::new(vec![b'x'; MAX_LINE + 10]);
        assert!(matches!(read_line(&mut long), Err(ReadError::TooLong)));
    }

    #[test]
    fn socket_paths() {
        assert!(socket_path(0).ends_with("glint/glintd-0.sock") || socket_path(0).ends_with("glintd-0.sock"));
    }
}
