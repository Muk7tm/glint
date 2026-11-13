// SPDX-License-Identifier: MIT
//! The desktop's own file dialog, through xdg-desktop-portal's FileChooser on the session bus
//! (PLAN §12.4 *Import* and *Export*, ADR 0019). Slint has no file dialog, and the portal is on
//! every current desktop: GNOME, KDE, LXQt, Xfce (with xdg-desktop-portal-gtk) and others.
//!
//! Blocking: run it on a thread of its own. It returns once the user chose or cancelled.

use std::collections::HashMap;
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt as _;
use std::path::PathBuf;

use zbus::blocking::{Connection, MessageIterator, Proxy};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

/// What to ask for.
pub enum Choose {
    /// A profile file to read.
    Open,
    /// Where to write a profile, suggesting `name`.
    Save { name: String },
}

/// Shows the dialog; `None` when the user cancelled.
pub fn choose(what: &Choose) -> Result<Option<PathBuf>, String> {
    let conn = Connection::session().map_err(|e| format!("no session bus ({e})"))?;
    // The portal answers on a request object whose path it derives from our bus name and a token
    // we choose; listening there before asking means the answer can't be missed.
    let sender = conn
        .unique_name()
        .ok_or("no D-Bus name")?
        .as_str()
        .trim_start_matches(':')
        .replace('.', "_");
    let token = format!("glint{}", std::process::id());
    let path = format!("/org/freedesktop/portal/desktop/request/{sender}/{token}");
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface("org.freedesktop.portal.Request")
        .and_then(|b| b.member("Response"))
        .and_then(|b| b.path(path.as_str()))
        .map_err(|e| e.to_string())?
        .build();
    let mut answers = MessageIterator::for_match_rule(rule, &conn, Some(4)).map_err(|e| e.to_string())?;
    let chooser = Proxy::new(
        &conn,
        "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.FileChooser",
    )
    .map_err(|e| e.to_string())?;
    let mut options: HashMap<&str, Value> = HashMap::new();
    options.insert("handle_token", Value::from(token.as_str()));
    options.insert("modal", Value::from(true));
    options.insert(
        "filters",
        Value::new(vec![("Glint profiles (TOML)", vec![(0u32, "*.toml")])]),
    );
    let (method, title) = match what {
        Choose::Open => ("OpenFile", "Import a Glint profile"),
        Choose::Save { name } => {
            options.insert("current_name", Value::from(name.as_str()));
            ("SaveFile", "Export this Glint profile")
        }
    };
    let _request: OwnedObjectPath = chooser
        .call(method, &("", title, options))
        .map_err(|e| format!("no file dialog ({e})"))?;
    let answer = answers
        .next()
        .ok_or("the file dialog went away")?
        .map_err(|e| e.to_string())?;
    let (response, mut results): (u32, HashMap<String, OwnedValue>) =
        answer.body().deserialize().map_err(|e| e.to_string())?;
    if response != 0 {
        return Ok(None);
    }
    let uris = results
        .remove("uris")
        .and_then(|v| Vec::<String>::try_from(v).ok())
        .unwrap_or_default();
    Ok(uris.first().and_then(|uri| file_path(uri)))
}

/// The path of a `file://` URI, with its `%XX` escapes undone.
fn file_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hex) = rest.get(i + 1..i + 3)
            && let Ok(b) = u8::from_str_radix(hex, 16)
        {
            out.push(b);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    Some(PathBuf::from(OsString::from_vec(out)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_uris_become_paths() {
        assert_eq!(
            file_path("file:///home/me/My%20Profiles/sober%E2%80%94green.toml"),
            Some(PathBuf::from("/home/me/My Profiles/sober—green.toml"))
        );
        assert_eq!(file_path("https://example.com/x.toml"), None);
    }
}
