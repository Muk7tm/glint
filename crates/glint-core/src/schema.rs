// SPDX-License-Identifier: MIT
//! Schema versions and migrations for Glint's TOML files (PLAN §11).
//!
//! Every file carries `schema_version`. An older file is migrated in memory, one version at a
//! time, and the caller (the engine's store) keeps a `.bak` copy of the original before saving
//! the new version. A file from a newer Glint is refused rather than guessed at.

use serde::de::DeserializeOwned;

/// Upgrades a parsed file from one schema version to the next, in place.
pub type Migration = fn(&mut toml::Table) -> Result<(), String>;

/// A file that was read, and the schema it was migrated from, if it was. After a migration the
/// caller writes a `.bak` copy of the original and saves the new version.
#[derive(Clone, PartialEq, Debug)]
pub struct Loaded<T> {
    pub value: T,
    pub migrated_from: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum LoadError {
    #[error("{0}")]
    Toml(#[from] toml::de::Error),
    #[error("schema_version is missing")]
    MissingVersion,
    #[error("schema_version must be a whole number")]
    BadVersion,
    #[error("this file is from a newer Glint (schema {found}; this one reads up to {supported}). Update Glint")]
    TooNew { found: i64, supported: u32 },
    #[error("schema {found} is too old to read (the oldest supported is {oldest})")]
    TooOld { found: i64, oldest: u32 },
    #[error("migrating from schema {from}: {message}")]
    Migration { from: u32, message: String },
}

/// Reads `text` as schema `current`. `migrations[i]` upgrades schema `current − len + i` to the
/// next one, so the oldest readable schema is `current − migrations.len()`.
pub fn load<T: DeserializeOwned>(text: &str, current: u32, migrations: &[Migration]) -> Result<Loaded<T>, LoadError> {
    let mut table: toml::Table = toml::from_str(text)?;
    let found = table
        .get("schema_version")
        .ok_or(LoadError::MissingVersion)?
        .as_integer()
        .ok_or(LoadError::BadVersion)?;
    let oldest = current.saturating_sub(u32::try_from(migrations.len()).unwrap_or(u32::MAX));
    if found > i64::from(current) {
        return Err(LoadError::TooNew {
            found,
            supported: current,
        });
    }
    if found < i64::from(oldest.max(1)) {
        return Err(LoadError::TooOld {
            found,
            oldest: oldest.max(1),
        });
    }
    if found == i64::from(current) {
        // Straight from the text, so errors keep their line and column.
        return Ok(Loaded {
            value: toml::from_str(text)?,
            migrated_from: None,
        });
    }
    let from = found as u32; // between `oldest` and `current`, checked above
    for version in from..current {
        let migrate = migrations[(version - oldest) as usize];
        migrate(&mut table).map_err(|message| LoadError::Migration { from: version, message })?;
        table.insert(
            "schema_version".to_owned(),
            toml::Value::Integer(i64::from(version + 1)),
        );
    }
    Ok(Loaded {
        value: table.try_into()?,
        migrated_from: Some(from),
    })
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;

    /// Schema 3 of a made-up file: schema 1 called `size` `old_size`, schema 2 had no `flag`.
    #[derive(Deserialize, PartialEq, Debug)]
    #[serde(deny_unknown_fields)]
    struct Doc {
        schema_version: u32,
        size: i64,
        flag: bool,
    }

    fn rename_size(t: &mut toml::Table) -> Result<(), String> {
        let v = t.remove("old_size").ok_or("old_size is missing")?;
        t.insert("size".to_owned(), v);
        Ok(())
    }

    fn add_flag(t: &mut toml::Table) -> Result<(), String> {
        t.insert("flag".to_owned(), toml::Value::Boolean(true));
        Ok(())
    }

    const CHAIN: &[Migration] = &[rename_size, add_flag];

    #[test]
    fn current_files_load_unchanged() {
        let doc: Loaded<Doc> = load("schema_version = 3\nsize = 7\nflag = false", 3, CHAIN).unwrap();
        assert_eq!(
            doc,
            Loaded {
                value: Doc {
                    schema_version: 3,
                    size: 7,
                    flag: false
                },
                migrated_from: None
            }
        );
    }

    #[test]
    fn older_files_migrate_one_version_at_a_time() {
        let doc: Loaded<Doc> = load("schema_version = 1\nold_size = 7", 3, CHAIN).unwrap();
        assert_eq!(
            doc,
            Loaded {
                value: Doc {
                    schema_version: 3,
                    size: 7,
                    flag: true
                },
                migrated_from: Some(1)
            }
        );
        let doc: Loaded<Doc> = load("schema_version = 2\nsize = 7", 3, CHAIN).unwrap();
        assert_eq!(doc.migrated_from, Some(2));
    }

    #[test]
    fn refused_files() {
        let err = |text: &str| load::<Doc>(text, 3, CHAIN).unwrap_err();
        assert_eq!(err("size = 7"), LoadError::MissingVersion);
        assert_eq!(err("schema_version = \"3\""), LoadError::BadVersion);
        assert_eq!(err("schema_version = 4"), LoadError::TooNew { found: 4, supported: 3 });
        assert_eq!(err("schema_version = 0"), LoadError::TooOld { found: 0, oldest: 1 });
        assert_eq!(
            err("schema_version = 1"),
            LoadError::Migration {
                from: 1,
                message: "old_size is missing".into()
            }
        );
        assert!(matches!(
            err("schema_version = 3\nsize = 7\nflag = true\nextra = 1"),
            LoadError::Toml(_)
        ));
        assert!(matches!(err("schema_version = "), LoadError::Toml(_)));
        // With no migrations, only the current schema is readable.
        assert_eq!(
            load::<Doc>("schema_version = 2", 3, &[]).unwrap_err(),
            LoadError::TooOld { found: 2, oldest: 3 }
        );
    }

    #[test]
    fn errors_name_the_line_and_field() {
        let message = load::<Doc>("schema_version = 3\nsize = 7\nflag = true\ncolour = 1\n", 3, CHAIN)
            .unwrap_err()
            .to_string();
        assert!(message.contains("colour") && message.contains("line 4"), "{message}");
    }
}
