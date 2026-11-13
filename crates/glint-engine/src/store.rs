// SPDX-License-Identifier: MIT
//! Settings and profiles on disk (PLAN §11): `<config>/settings.toml` and
//! `<config>/profiles/<name>.toml`, where `<config>` is `$XDG_CONFIG_HOME/glint` or
//! `~/.config/glint`.
//!
//! The engine is the only writer. Writes are debounced (300 ms after the last change) and atomic
//! (a temporary file, `fsync`, `rename`, then an `fsync` of the directory), so holding a stepper
//! button doesn't hammer the disk and a crash never leaves half a file. An older schema is
//! migrated in memory; the original is kept as `<file>.bak` and the new version is saved. A file
//! that can't be read is left untouched and reported, never overwritten.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use glint_core::profile::Profile;
use glint_core::settings::Settings;
use glint_core::validate;

/// How long after the last change a file is written (§11).
pub const DEBOUNCE: Duration = Duration::from_millis(300);

/// `$XDG_CONFIG_HOME/glint`, or `~/.config/glint`.
pub fn config_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"));
    base.join("glint")
}

/// A profile's file name: its name in lowercase ASCII, with every run of other characters as one
/// dash ("Sober — green flash" → `sober-green-flash.toml`, Appendix B).
pub fn file_name(profile_name: &str) -> String {
    let mut slug = String::new();
    for c in profile_name.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-');
    format!("{}.toml", if slug.is_empty() { "profile" } else { slug })
}

/// What was on disk.
#[derive(Debug, Default)]
pub struct Loaded {
    pub settings: Settings,
    pub profiles: BTreeMap<String, Profile>,
    /// Files that couldn't be used, and why, for the log and diagnostics.
    pub problems: Vec<String>,
}

pub struct Store {
    dir: PathBuf,
    /// Profile name → its file, so a profile keeps its file even if another would collide.
    files: BTreeMap<String, PathBuf>,
    pending: BTreeMap<PathBuf, (String, Instant)>,
}

impl Store {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            files: BTreeMap::new(),
            pending: BTreeMap::new(),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Reads settings and every profile, migrating older schemas. Always returns something usable:
    /// defaults stand in for whatever couldn't be read, and the active profile always exists.
    pub fn load(&mut self) -> Loaded {
        let mut out = Loaded::default();
        let settings_path = self.dir.join("settings.toml");
        match std::fs::read_to_string(&settings_path) {
            Ok(text) => match Settings::load(&text) {
                Ok(loaded) if validate::settings(&loaded.value).is_ok() => {
                    out.settings = loaded.value;
                    if loaded.migrated_from.is_some() {
                        self.migrated(&settings_path, &text, &mut out.problems);
                        self.save_settings(&out.settings, Instant::now());
                    }
                }
                Ok(loaded) => {
                    let issue = &validate::settings(&loaded.value).errors[0];
                    out.problems.push(format!(
                        "{}: {}: {}; using the defaults",
                        settings_path.display(),
                        issue.field,
                        issue.message
                    ));
                }
                Err(e) => out
                    .problems
                    .push(format!("{}: {e}; using the defaults", settings_path.display())),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => out.problems.push(format!("{}: {e}", settings_path.display())),
        }

        let profiles_dir = self.dir.join("profiles");
        let mut paths: Vec<PathBuf> = std::fs::read_dir(&profiles_dir)
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|x| x == "toml"))
                    .collect()
            })
            .unwrap_or_default();
        paths.sort();
        for path in paths {
            let text = match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(e) => {
                    out.problems.push(format!("{}: {e}", path.display()));
                    continue;
                }
            };
            match Profile::load(&text) {
                Ok(loaded) => {
                    let report = validate::profile(&loaded.value);
                    if let Some(issue) = report.errors.first() {
                        out.problems.push(format!(
                            "{}: {}: {}; skipped",
                            path.display(),
                            issue.field,
                            issue.message
                        ));
                        continue;
                    }
                    let name = loaded.value.name.clone();
                    if out.profiles.contains_key(&name) {
                        out.problems.push(format!(
                            "{}: another file already has the profile {name:?}; skipped",
                            path.display()
                        ));
                        continue;
                    }
                    self.files.insert(name.clone(), path.clone());
                    if loaded.migrated_from.is_some() {
                        self.migrated(&path, &text, &mut out.problems);
                        self.save_profile(&loaded.value, Instant::now());
                    }
                    out.profiles.insert(name, loaded.value);
                }
                Err(e) => out.problems.push(format!("{}: {e}; skipped", path.display())),
            }
        }
        let active = out.settings.active_profile.clone();
        out.profiles
            .entry(active.clone())
            .or_insert_with(|| Profile::new(active));
        out
    }

    /// Keeps the original of a migrated file as `<file>.bak`.
    fn migrated(&self, path: &Path, original: &str, problems: &mut Vec<String>) {
        let mut bak = path.as_os_str().to_owned();
        bak.push(".bak");
        if let Err(e) = std::fs::write(&bak, original) {
            problems.push(format!(
                "{}: couldn't keep a backup before migrating: {e}",
                path.display()
            ));
        }
    }

    /// Schedules `settings` to be written once nothing has changed for [`DEBOUNCE`].
    pub fn save_settings(&mut self, settings: &Settings, now: Instant) {
        if let Ok(text) = settings.to_toml() {
            self.pending
                .insert(self.dir.join("settings.toml"), (text, now + DEBOUNCE));
        }
    }

    /// Schedules `profile` to be written to its file once nothing has changed for [`DEBOUNCE`].
    pub fn save_profile(&mut self, profile: &Profile, now: Instant) {
        let path = match self.files.get(&profile.name) {
            Some(p) => p.clone(),
            None => {
                let path = self.free_path(&profile.name);
                self.files.insert(profile.name.clone(), path.clone());
                path
            }
        };
        if let Ok(text) = profile.to_toml() {
            self.pending.insert(path, (text, now + DEBOUNCE));
        }
    }

    /// A file for a new profile that no other profile uses.
    fn free_path(&self, name: &str) -> PathBuf {
        let base = file_name(name);
        let stem = base.trim_end_matches(".toml");
        let taken = |p: &PathBuf| self.files.values().any(|f| f == p) || p.exists();
        let first = self.dir.join("profiles").join(&base);
        if !taken(&first) {
            return first;
        }
        (2..)
            .map(|n| self.dir.join("profiles").join(format!("{stem}-{n}.toml")))
            .find(|p| !taken(p))
            .unwrap_or(first)
    }

    /// Writes every file whose debounce has run out; returns what failed.
    pub fn flush_due(&mut self, now: Instant) -> Vec<String> {
        let due: Vec<PathBuf> = self
            .pending
            .iter()
            .filter(|(_, (_, at))| *at <= now)
            .map(|(p, _)| p.clone())
            .collect();
        self.write(due)
    }

    /// Writes everything still pending (at shutdown).
    pub fn flush_all(&mut self) -> Vec<String> {
        let all: Vec<PathBuf> = self.pending.keys().cloned().collect();
        self.write(all)
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    fn write(&mut self, paths: Vec<PathBuf>) -> Vec<String> {
        let mut problems = Vec::new();
        for path in paths {
            if let Some((text, _)) = self.pending.remove(&path)
                && let Err(e) = write_atomically(&path, &text)
            {
                problems.push(format!("{}: {e}", path.display()));
            }
        }
        problems
    }
}

/// Writes `text` to `path` so that a crash leaves either the old file or the new one.
pub fn write_atomically(path: &Path, text: &str) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".tmp-{}", std::process::id()));
    let tmp = PathBuf::from(tmp);
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    std::fs::File::open(dir)?.sync_all()
}

#[cfg(test)]
mod tests {
    use glint_core::color::{ColorModel, ShadePreset};
    use glint_core::{Rgb, settings::ClosePolicy};

    use super::*;

    struct Dir(PathBuf);

    impl Dir {
        fn new(tag: &str) -> Self {
            let d = std::env::temp_dir().join(format!("glint-store-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            Self(d)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn green(name: &str) -> Profile {
        let mut p = Profile::new(name);
        p.color.include = vec![ShadePreset::Normal.entry(ColorModel::Oklch, Rgb::new(0x3B, 0xD1, 0x6F))];
        p
    }

    #[test]
    fn file_names() {
        assert_eq!(file_name("Sober — green flash"), "sober-green-flash.toml");
        assert_eq!(file_name("Default"), "default.toml");
        assert_eq!(file_name("  ..//  "), "profile.toml");
        assert_eq!(file_name("a/../b"), "a-b.toml");
    }

    #[test]
    fn an_empty_directory_gives_the_defaults() {
        let d = Dir::new("empty");
        let loaded = Store::new(d.0.join("glint")).load();
        assert_eq!(loaded.settings, Settings::default());
        assert_eq!(loaded.profiles.keys().collect::<Vec<_>>(), ["Default"]);
        assert!(loaded.problems.is_empty());
    }

    #[test]
    fn saves_are_debounced_atomic_and_load_back() {
        let d = Dir::new("roundtrip");
        let mut store = Store::new(d.0.clone());
        let t0 = Instant::now();
        let settings = Settings {
            active_profile: "Sober — green flash".into(),
            close_policy: ClosePolicy::Quit,
            ..Settings::default()
        };
        store.save_settings(&settings, t0);
        store.save_profile(&green("Sober — green flash"), t0);
        // A burst of edits: only the last one is written, 300 ms after it.
        let mut later = green("Sober — green flash");
        later.action.hold_ms = 40;
        store.save_profile(&later, t0 + Duration::from_millis(200));
        assert!(store.flush_due(t0 + Duration::from_millis(350)).is_empty());
        assert!(d.0.join("settings.toml").exists());
        assert!(!d.0.join("profiles/sober-green-flash.toml").exists()); // due at 500 ms
        assert!(store.flush_due(t0 + Duration::from_millis(500)).is_empty());
        assert!(!store.has_pending());
        let names: Vec<_> = std::fs::read_dir(d.0.join("profiles"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(names, ["sober-green-flash.toml"]); // no temporary files left behind

        let loaded = Store::new(d.0.clone()).load();
        assert!(loaded.problems.is_empty(), "{:?}", loaded.problems);
        assert_eq!(loaded.settings, settings);
        assert_eq!(loaded.profiles["Sober — green flash"], later);
    }

    #[test]
    fn bad_files_are_reported_and_left_alone() {
        let d = Dir::new("bad");
        std::fs::create_dir_all(d.0.join("profiles")).unwrap();
        std::fs::write(d.0.join("settings.toml"), "schema_version = 99\n").unwrap();
        std::fs::write(
            d.0.join("profiles/typo.toml"),
            "schema_version = 1\nname = \"Typo\"\ncolour = 1\n",
        )
        .unwrap();
        std::fs::write(
            d.0.join("profiles/invalid.toml"),
            "schema_version = 1\nname = \"Bad\"\n[trigger]\nmax_clicks_per_sec = 0\n",
        )
        .unwrap();
        std::fs::write(d.0.join("profiles/a.toml"), green("Same").to_toml().unwrap()).unwrap();
        std::fs::write(d.0.join("profiles/b.toml"), green("Same").to_toml().unwrap()).unwrap();
        let loaded = Store::new(d.0.clone()).load();
        assert_eq!(loaded.settings, Settings::default());
        assert_eq!(loaded.profiles.keys().collect::<Vec<_>>(), ["Default", "Same"]);
        assert_eq!(loaded.problems.len(), 4, "{:?}", loaded.problems);
        assert!(loaded.problems[0].contains("newer Glint"), "{:?}", loaded.problems);
        // Nothing was rewritten.
        assert_eq!(
            std::fs::read_to_string(d.0.join("settings.toml")).unwrap(),
            "schema_version = 99\n"
        );
    }

    #[test]
    fn colliding_file_names_get_a_number() {
        let d = Dir::new("collide");
        let mut store = Store::new(d.0.clone());
        let t0 = Instant::now();
        store.save_profile(&green("Green!"), t0);
        store.save_profile(&green("green?"), t0);
        store.flush_all();
        assert!(d.0.join("profiles/green.toml").exists() && d.0.join("profiles/green-2.toml").exists());
        let loaded = Store::new(d.0.clone()).load();
        assert!(loaded.profiles.contains_key("Green!") && loaded.profiles.contains_key("green?"));
    }
}
