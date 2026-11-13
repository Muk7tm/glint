// SPDX-License-Identifier: MIT
//! *Start at login* (More options → ENGINE, PLAN §18): an XDG autostart entry that starts the
//! engine, without the window, when the user logs in. The entry's presence is the setting. It is
//! off until the user turns it on, and the engine it starts never starts watching on its own.

use std::io;
use std::path::{Path, PathBuf};

/// The entry, with `Exec` still to be set to the engine's full path.
const TEMPLATE: &str = include_str!("../../../packaging/autostart/glintd.desktop");

/// Where the entry lives: `$XDG_CONFIG_HOME/autostart/glintd.desktop`, else under `~/.config`.
pub fn entry_path() -> Option<PathBuf> {
    // The XDG spec: an empty or relative XDG_CONFIG_HOME counts as unset.
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .map(|home| home.join(".config"))
        })?;
    Some(config.join("autostart").join("glintd.desktop"))
}

/// Whether the engine starts at login.
pub fn is_on(entry: &Path) -> bool {
    entry.is_file()
}

/// Turns starting at login on, with an entry that runs `engine`, or off, by removing the entry.
pub fn set(entry: &Path, on: bool, engine: &Path) -> io::Result<()> {
    if !on {
        return match std::fs::remove_file(entry) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            other => other,
        };
    }
    let text = contents(engine)?;
    if let Some(dir) = entry.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Write, then rename over the old entry, so a session that starts meanwhile never reads half
    // of it.
    let fresh = entry.with_extension("desktop.new");
    std::fs::write(&fresh, text)?;
    std::fs::rename(&fresh, entry)
}

/// The entry's text: the template, with `Exec` set to `engine` and the license line left out.
fn contents(engine: &Path) -> io::Result<String> {
    let engine = engine
        .to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "the engine's path isn't UTF-8"))?;
    let exec = format!("Exec={}", exec_arg(engine));
    let mut text = String::with_capacity(TEMPLATE.len() + exec.len());
    for line in TEMPLATE.lines().filter(|l| !l.starts_with("# SPDX")) {
        text.push_str(if line.starts_with("Exec=") { &exec } else { line });
        text.push('\n');
    }
    Ok(text)
}

/// One argument of a desktop entry's `Exec` key (Desktop Entry Specification, "The Exec key"):
/// `%` doubled, and quoted when it holds a reserved character. Inside the quotes, `"`, `` ` ``,
/// `$` and `\` take a backslash, which itself is written `\\` because the file's string escapes
/// are undone first.
fn exec_arg(arg: &str) -> String {
    const RESERVED: &[char] = &[
        ' ', '\t', '\n', '\r', '"', '\'', '\\', '>', '<', '~', '|', '&', ';', '$', '*', '?', '#', '(', ')', '`',
    ];
    let arg = arg.replace('%', "%%");
    if !arg.contains(RESERVED) {
        return arg;
    }
    let mut out = String::with_capacity(arg.len() + 8);
    out.push('"');
    for c in arg.chars() {
        match c {
            '"' | '`' | '$' => {
                out.push_str(r"\\");
                out.push(c);
            }
            '\\' => out.push_str(r"\\\\"),
            '\n' => out.push_str(r"\n"),
            '\t' => out.push_str(r"\t"),
            '\r' => out.push_str(r"\r"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_paths_stay_as_they_are_and_others_are_quoted() {
        assert_eq!(exec_arg("/home/me/.local/bin/glintd"), "/home/me/.local/bin/glintd");
        assert_eq!(exec_arg("/home/Jo Doe/bin/glintd"), r#""/home/Jo Doe/bin/glintd""#);
        assert_eq!(exec_arg("/opt/100%/glintd"), "/opt/100%%/glintd");
        assert_eq!(exec_arg(r#"/tmp/a$b"c\d%e`f"#), r#""/tmp/a\\$b\\"c\\\\d%%e\\`f""#);
    }

    #[test]
    fn the_entry_runs_the_engine_by_its_full_path() {
        let text = contents(Path::new("/home/Jo Doe/.local/bin/glintd")).unwrap();
        assert!(text.starts_with("# Start at login"), "{text}");
        assert!(text.contains("\n[Desktop Entry]\nType=Application\n"), "{text}");
        assert!(text.contains("\nExec=\"/home/Jo Doe/.local/bin/glintd\"\n"), "{text}");
        assert_eq!(text.matches("Exec=").count(), 1, "{text}");
        assert!(!text.contains("SPDX"), "{text}");
    }

    #[test]
    fn on_writes_the_entry_and_off_removes_it() {
        let dir = std::env::temp_dir().join(format!("glint-autostart-test-{}", std::process::id()));
        let entry = dir.join("autostart").join("glintd.desktop");
        let engine = Path::new("/usr/bin/glintd");
        assert!(!is_on(&entry));
        set(&entry, true, engine).unwrap();
        assert!(is_on(&entry));
        let text = std::fs::read_to_string(&entry).unwrap();
        assert!(text.contains("\nExec=/usr/bin/glintd\n"), "{text}");
        // Turning it on again rewrites it; nothing is left beside it.
        set(&entry, true, engine).unwrap();
        let names: Vec<_> = std::fs::read_dir(entry.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["glintd.desktop"]);
        set(&entry, false, engine).unwrap();
        assert!(!is_on(&entry));
        // Off when already off is fine.
        set(&entry, false, engine).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
