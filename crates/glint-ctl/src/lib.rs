// SPDX-License-Identifier: MIT
//! `glint-ctl`: the engine from the command line (PLAN §10). Everything goes through the engine's
//! socket, except `release-all`, which also works when no engine answers (C10).
#![forbid(unsafe_code)]

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};
use glint_core::{Profile, Rgb};
use glint_ipc::{Client, ClientKind, Event, Request, StateSnapshot, TargetKind};
use glint_x11::env::LocalDisplay;

const WAIT: Duration = Duration::from_secs(5);

#[derive(Parser, Debug)]
#[command(name = "glint-ctl", version, about = "Control the Glint engine (MIT)")]
pub struct Cli {
    /// The engine's socket (default: the one for $DISPLAY).
    #[arg(long, global = true)]
    pub socket: Option<PathBuf>,
    /// The X display (default: $DISPLAY).
    #[arg(long, global = true)]
    pub display: Option<String>,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Whether the engine runs, and what it's doing.
    Status,
    /// Start watching.
    Start,
    /// Stop watching.
    Stop,
    /// Start or stop.
    Toggle,
    /// Emergency stop: stop watching and release everything.
    Panic,
    /// Release every button and key Glint could hold, even when no engine runs.
    ReleaseAll,
    /// Stop the engine.
    Quit,
    /// Profiles.
    #[command(subcommand)]
    Profile(ProfileCommand),
    /// How the active profile classifies colors: `glint-ctl classify '#3BD16F'`.
    Classify { colors: Vec<String> },
    /// Everything about the engine, for bug reports.
    Diag,
}

#[derive(Subcommand, Debug)]
pub enum ProfileCommand {
    List,
    /// Make a profile the active one.
    Use {
        name: String,
    },
    /// Print a profile (the active one by default) as TOML.
    Show {
        name: Option<String>,
    },
    /// Print a profile as TOML, to save it as a file.
    Export {
        name: String,
    },
    /// Load a profile file and make it active.
    Import {
        file: PathBuf,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum CtlError {
    #[error(transparent)]
    Display(#[from] glint_x11::env::DisplayError),
    #[error("no engine is running for this display ({0})")]
    NoEngine(String),
    #[error(transparent)]
    Read(#[from] glint_ipc::ReadError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Engine(String),
    #[error("{0}")]
    Profile(String),
    #[error("releasing without an engine: {0}")]
    Release(String),
    #[error("the engine answered with an unexpected {0}")]
    Unexpected(String),
}

/// Runs one command, writing its output to `out`.
pub fn execute(cli: &Cli, out: &mut dyn Write) -> Result<(), CtlError> {
    let display = match &cli.display {
        Some(d) => LocalDisplay::parse(d)?,
        None => LocalDisplay::from_env()?,
    };
    let socket = cli
        .socket
        .clone()
        .unwrap_or_else(|| glint_ipc::socket_path(display.number));
    let client = Client::connect(&socket).ok();
    let Some(mut client) = client else {
        if matches!(cli.command, Command::ReleaseAll) {
            release_without_engine(display)?;
            writeln!(
                out,
                "No engine is running; released every button and key through XTest directly."
            )?;
            return Ok(());
        }
        return Err(CtlError::NoEngine(socket.display().to_string()));
    };
    client.hello(ClientKind::Ctl, WAIT)?;
    let reply = |client: &mut Client, r: Request| -> Result<Event, CtlError> {
        match client.call(r, WAIT)? {
            Event::Error { message, field, .. } => Err(CtlError::Engine(match field {
                Some(f) => format!("{f}: {message}"),
                None => message,
            })),
            e => Ok(e),
        }
    };
    match &cli.command {
        Command::Status => {
            let state = expect_state(reply(&mut client, Request::GetState)?)?;
            let pid = client.peer_pid().map_or_else(String::new, |p| format!(" (pid {p})"));
            writeln!(out, "Engine running{pid} on {}", display.canonical())?;
            write_state(out, &state)?;
        }
        Command::Start => write_state(out, &expect_state(reply(&mut client, Request::Start)?)?)?,
        Command::Stop => write_state(out, &expect_state(reply(&mut client, Request::Stop)?)?)?,
        Command::Toggle => write_state(out, &expect_state(reply(&mut client, Request::Toggle)?)?)?,
        Command::Panic => write_state(out, &expect_state(reply(&mut client, Request::EmergencyStop)?)?)?,
        Command::ReleaseAll => {
            reply(&mut client, Request::ReleaseAll)?;
            writeln!(out, "Released every button and key.")?;
        }
        Command::Quit => {
            reply(&mut client, Request::Quit)?;
            writeln!(out, "The engine stopped.")?;
        }
        Command::Profile(ProfileCommand::List) => match reply(&mut client, Request::ListProfiles)? {
            Event::Profiles { names, active } => {
                for n in names {
                    writeln!(out, "{} {n}", if n == active { "*" } else { " " })?;
                }
            }
            e => return Err(unexpected(&e)),
        },
        Command::Profile(ProfileCommand::Use { name }) => {
            write_state(
                out,
                &expect_state(reply(&mut client, Request::UseProfile { name: name.clone() })?)?,
            )?;
        }
        Command::Profile(ProfileCommand::Show { name }) => {
            write_profile(out, reply(&mut client, Request::GetProfile { name: name.clone() })?)?
        }
        Command::Profile(ProfileCommand::Export { name }) => {
            write_profile(
                out,
                reply(
                    &mut client,
                    Request::GetProfile {
                        name: Some(name.clone()),
                    },
                )?,
            )?;
        }
        Command::Profile(ProfileCommand::Import { file }) => {
            let text = std::fs::read_to_string(file)?;
            let profile = Profile::load(&text)
                .map_err(|e| CtlError::Profile(format!("{}: {e}", file.display())))?
                .value;
            let name = profile.name.clone();
            match reply(
                &mut client,
                Request::SetProfile {
                    profile: Box::new(profile),
                },
            )? {
                Event::ProfileApplied { rev } => {
                    writeln!(out, "Imported {name:?} (revision {rev}); it's the active profile.")?
                }
                e => return Err(unexpected(&e)),
            }
        }
        Command::Classify { colors } => {
            let parsed: Vec<Rgb> = colors
                .iter()
                .map(|c| c.parse::<Rgb>())
                .collect::<Result<_, _>>()
                .map_err(|e| CtlError::Engine(e.to_string()))?;
            match reply(&mut client, Request::Classify { colors: parsed.clone() })? {
                Event::Classified { verdicts } => {
                    for (c, v) in parsed.iter().zip(verdicts) {
                        writeln!(
                            out,
                            "{c} {}",
                            serde_json::to_string(&v).unwrap_or_default().trim_matches('"')
                        )?;
                    }
                }
                e => return Err(unexpected(&e)),
            }
        }
        Command::Diag => {
            // One text block for bug reports, the same as the window's *Copy diagnostics* (§17).
            writeln!(out, "glint-ctl {} · MIT", env!("CARGO_PKG_VERSION"))?;
            match reply(&mut client, Request::Diagnostics)? {
                Event::Diagnostics { text } => write!(out, "{text}")?,
                e => return Err(unexpected(&e)),
            }
        }
    }
    Ok(())
}

/// The command line's entry point.
pub fn run(cli: &Cli) -> ExitCode {
    let mut out = std::io::stdout();
    match execute(cli, &mut out) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e @ CtlError::NoEngine(_)) => {
            eprintln!("glint-ctl: {e}");
            ExitCode::from(2)
        }
        Err(e) => {
            eprintln!("glint-ctl: {e}");
            ExitCode::FAILURE
        }
    }
}

fn unexpected(e: &Event) -> CtlError {
    CtlError::Unexpected(serde_json::to_string(e).unwrap_or_default())
}

fn expect_state(e: Event) -> Result<StateSnapshot, CtlError> {
    match e {
        Event::State(s) => Ok(s),
        e => Err(unexpected(&e)),
    }
}

fn write_state(out: &mut dyn Write, s: &StateSnapshot) -> Result<(), CtlError> {
    let what = if s.started { "watching" } else { "stopped" };
    let rate = s
        .checks_per_sec
        .map_or_else(|| "max speed".to_owned(), |n| format!("{n} checks/s"));
    let target = match s.target.kind {
        TargetKind::Screen => "monitor",
        TargetKind::Window => "window",
    };
    writeln!(
        out,
        "{what} · profile {} (revision {}) · {target} {} · {rate}",
        s.profile, s.revision, s.target.name
    )?;
    match &s.backend_reason {
        Some(why) => writeln!(out, "reads: {} ({why})", s.backend)?,
        None => writeln!(out, "reads: {}", s.backend)?,
    }
    if let (Some(realtime), Some(cpu)) = (&s.realtime, &s.cpu) {
        writeln!(out, "real-time: {realtime} · runs on {cpu}")?;
    }
    if let Some(why) = &s.last_stop {
        writeln!(out, "last stop: {why}")?;
    }
    if let Some(why) = &s.cannot_start {
        writeln!(out, "can't start yet: {why}")?;
    }
    Ok(())
}

fn write_profile(out: &mut dyn Write, e: Event) -> Result<(), CtlError> {
    match e {
        Event::Profile { profile } => {
            let text = profile.to_toml().map_err(|e| CtlError::Profile(e.to_string()))?;
            write!(out, "{text}")?;
            Ok(())
        }
        e => Err(unexpected(&e)),
    }
}

/// Releases every mouse button Glint can press and every keycode, held or not, directly through
/// XTest: for when the engine is gone and something is stuck (C10). A release of something that
/// isn't held does nothing.
pub fn release_without_engine(display: LocalDisplay) -> Result<(), CtlError> {
    use x11rb::connection::Connection as _;
    use x11rb::wrapper::ConnectionExt as _;
    let conn = glint_x11::conn::open(display).map_err(|e| CtlError::Release(e.to_string()))?;
    let root = conn
        .setup()
        .roots
        .get(usize::from(display.screen))
        .map_or(0, |s| s.root);
    let (min, max) = (conn.setup().min_keycode, conn.setup().max_keycode);
    let keycodes: Vec<u8> = (min..=max).collect();
    glint_x11::inject::release_everything_on(&conn, root, &keycodes).map_err(|e| CtlError::Release(e.to_string()))?;
    conn.sync().map_err(|e| CtlError::Release(e.to_string()))
}
