// SPDX-License-Identifier: MIT
//! Which X display Glint talks to, and what kind of session it is (PLAN §4.4, §8).
//!
//! Only local displays are supported: the hot connections speak the X protocol directly over
//! the display's Unix socket (ADR 0003), and remote X is out of scope anyway (§8).

use std::fmt;

/// A local X display, as `:N` or `:N.S`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct LocalDisplay {
    pub number: u16,
    pub screen: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DisplayError {
    #[error("DISPLAY is not set; Glint needs an X11 session")]
    Unset,
    #[error("DISPLAY={0:?} is not a local X display (Glint supports :N, :N.S and unix:N)")]
    NotLocal(String),
    #[error("DISPLAY={0:?} is malformed")]
    Malformed(String),
}

impl LocalDisplay {
    /// Parses `:N`, `:N.S`, `unix:N` and `unix:N.S`. Anything with a host or a protocol is remote.
    pub fn parse(name: &str) -> Result<Self, DisplayError> {
        let rest = name.strip_prefix("unix").filter(|r| r.starts_with(':')).unwrap_or(name);
        let Some(rest) = rest.strip_prefix(':') else {
            return Err(if name.contains(':') {
                DisplayError::NotLocal(name.to_owned())
            } else {
                DisplayError::Malformed(name.to_owned())
            });
        };
        let (number, screen) = rest.split_once('.').unwrap_or((rest, "0"));
        let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        if !digits(number) || !digits(screen) {
            return Err(DisplayError::Malformed(name.to_owned()));
        }
        Ok(Self {
            number: number.parse().map_err(|_| DisplayError::Malformed(name.to_owned()))?,
            screen: screen.parse().map_err(|_| DisplayError::Malformed(name.to_owned()))?,
        })
    }

    /// Reads `DISPLAY`.
    pub fn from_env() -> Result<Self, DisplayError> {
        match std::env::var("DISPLAY") {
            Ok(v) if !v.is_empty() => Self::parse(&v),
            _ => Err(DisplayError::Unset),
        }
    }

    /// The same display however it was written (`:0`, `:0.0`, `unix:0`): the key for the
    /// single-instance lock and the socket, so two spellings can't start two engines (C6).
    pub fn canonical(&self) -> String {
        format!(":{}", self.number)
    }

    /// The display's Unix socket.
    pub fn socket_path(&self) -> String {
        format!("/tmp/.X11-unix/X{}", self.number)
    }
}

impl fmt::Display for LocalDisplay {
    /// `:N.S`, a form every X library understands.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, ":{}.{}", self.number, self.screen)
    }
}

/// Hints that the session is Wayland with XWayland: only X11 apps can be watched and clicked (§8).
pub fn wayland_session() -> bool {
    std::env::var("XDG_SESSION_TYPE").is_ok_and(|v| v.eq_ignore_ascii_case("wayland"))
        || std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_spellings_share_one_canonical_name() {
        for name in [":0", ":0.0", "unix:0", "unix:0.1"] {
            let d = LocalDisplay::parse(name).unwrap();
            assert_eq!((d.number, d.canonical()), (0, ":0".to_owned()), "{name}");
        }
        let d = LocalDisplay::parse(":12.3").unwrap();
        assert_eq!((d.number, d.screen), (12, 3));
        assert_eq!(d.to_string(), ":12.3");
        assert_eq!(d.socket_path(), "/tmp/.X11-unix/X12");
    }

    #[test]
    fn remote_and_malformed_names() {
        for remote in ["localhost:10.0", "host:0", "tcp/host:0", "[::1]:0"] {
            assert_eq!(
                LocalDisplay::parse(remote),
                Err(DisplayError::NotLocal(remote.into())),
                "{remote}"
            );
        }
        for bad in [
            "",
            "0",
            ":",
            ":x",
            ":0.",
            ":0.x",
            ":-1",
            ":70000",
            "unix",
            "/tmp/.X11-unix/X0",
        ] {
            assert!(
                matches!(LocalDisplay::parse(bad), Err(DisplayError::Malformed(_))),
                "{bad}"
            );
        }
    }
}
