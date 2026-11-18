//! Smart playlists in and out as files, to carry their rules from one
//! device to another. A smart playlist is its name and its rules, so the
//! file holds those and nothing else: the songs come from whatever
//! library opens it.
//!
//! The file is TOML, one `[[smart]]` table per playlist, readable and easy
//! to edit by hand:
//!
//! ```toml
//! [[smart]]
//! name = "Blues I like"
//! rules = "genre is blues and rating >= 4"
//! exclude = "plays > 50"
//! ```
//!
//! `exclude` is left out when there are no songs to leave out. Rules are
//! the same text the smart playlist editor's text view shows.

use std::path::Path;

use rusqlite::params;
use serde::{Deserialize, Serialize};

use super::Library;
use super::smart::{SmartError, parse_with};

/// What a smart playlist file's name ends in.
pub const EXT: &str = ".smart.toml";

/// One smart playlist in a file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SmartEntry {
    pub name: String,
    pub rules: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub exclude: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct SmartFile {
    #[serde(default)]
    smart: Vec<SmartEntry>,
}

/// What an import made of a file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SmartImported {
    /// The names they were saved under, in the file's order.
    pub made: Vec<String>,
    /// How many of those took a new name, their own being taken.
    pub renamed: usize,
    /// Ones whose rules didn't read: their name and why.
    pub failed: Vec<(String, String)>,
}

/// A smart playlist file, by its name: anything ending in `.toml`.
pub fn is_smart_file(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("toml"))
}

/// Write smart playlists to `path`, replacing what's there.
pub fn write(path: &Path, entries: &[SmartEntry]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let file = SmartFile {
        smart: entries.to_vec(),
    };
    let body = toml::to_string(&file).map_err(std::io::Error::other)?;
    let text =
        format!("# PatinaPlayer smart playlists. Import this file in Playlists, Import.\n\n{body}");
    // Next to it first, then over it, so a failed write leaves nothing half done.
    let tmp = path.with_extension("toml.part");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

/// Read the smart playlists in a file. An error says what's wrong with it.
pub fn read(path: &Path) -> Result<Vec<SmartEntry>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let file: SmartFile =
        toml::from_str(&text).map_err(|e| format!("not a smart playlist file: {}", e.message()))?;
    if file.smart.is_empty() {
        return Err("there are no smart playlists in it".into());
    }
    Ok(file.smart)
}

impl Library {
    /// The smart playlists' names, for picking one that isn't taken.
    fn smart_names(&self) -> rusqlite::Result<Vec<String>> {
        let mut st = self
            .conn
            .prepare_cached("SELECT name FROM smart_playlists")?;
        st.query_map([], |r| r.get(0))?.collect()
    }

    /// Save the smart playlists in a file as new ones. One whose name is
    /// taken gets " 2", " 3" and so on after it; one whose rules don't
    /// read is left out and listed with why, and the rest still go in.
    pub fn import_smart(&self, entries: &[SmartEntry]) -> Result<SmartImported, SmartError> {
        let mut names = self.smart_names()?;
        let mut out = SmartImported::default();
        for e in entries {
            let base = e.name.trim();
            let base = if base.is_empty() { "Imported" } else { base };
            if let Err(err) = parse_with(&e.rules, &e.exclude) {
                out.failed.push((base.to_string(), err.to_string()));
                continue;
            }
            let taken = |n: &str| names.iter().any(|x| x.eq_ignore_ascii_case(n));
            let mut name = base.to_string();
            let mut k = 2;
            while taken(&name) {
                name = format!("{base} {k}");
                k += 1;
            }
            if name != base {
                out.renamed += 1;
            }
            let now = super::now_secs();
            self.conn.execute(
                "INSERT INTO smart_playlists (name, query, exclude, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?4)",
                params![name, e.rules.trim(), e.exclude.trim(), now],
            )?;
            names.push(name.clone());
            out.made.push(name);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, rules: &str, exclude: &str) -> SmartEntry {
        SmartEntry {
            name: name.into(),
            rules: rules.into(),
            exclude: exclude.into(),
        }
    }

    #[test]
    fn writes_and_reads_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(format!("Blues{EXT}"));
        let list = vec![
            entry(
                "Blues I like",
                "genre is blues and rating >= 4",
                "plays > 50",
            ),
            entry("Quote \"marks\"", "artist is \"Nirvana\"", ""),
        ];
        write(&path, &list).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# PatinaPlayer smart playlists"));
        // No exclude line when there's nothing to leave out.
        assert_eq!(text.matches("exclude").count(), 1);
        assert_eq!(read(&path).unwrap(), list);
        assert!(is_smart_file(&path));
        assert!(!is_smart_file(Path::new("x.m3u8")));
    }

    #[test]
    fn a_file_that_isnt_one_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("bad.toml");
        std::fs::write(&bad, "#EXTM3U\n/music/a.flac\n").unwrap();
        assert!(
            read(&bad)
                .unwrap_err()
                .starts_with("not a smart playlist file")
        );
        let empty = dir.path().join("empty.toml");
        std::fs::write(&empty, "# nothing\n").unwrap();
        assert_eq!(
            read(&empty).unwrap_err(),
            "there are no smart playlists in it"
        );
    }

    #[test]
    fn imports_with_free_names_and_skips_bad_rules() {
        let dir = tempfile::tempdir().unwrap();
        let lib = Library::open(&dir.path().join("lib.db")).unwrap();
        lib.create_smart("Loud", "rating >= 4", "").unwrap();
        let r = lib
            .import_smart(&[
                entry("Loud", "rating >= 5", ""),
                entry("loud", "rating >= 3", ""),
                entry("Broken", "rating is banana and", ""),
                entry("Quiet", "rating <= 1", "plays > 3"),
            ])
            .unwrap();
        assert_eq!(r.made, vec!["Loud 2", "loud 3", "Quiet"]);
        assert_eq!(r.renamed, 2);
        assert_eq!(r.failed.len(), 1);
        assert_eq!(r.failed[0].0, "Broken");
        let all = lib.smart_playlists().unwrap();
        let quiet = all.iter().find(|s| s.name == "Quiet").unwrap();
        assert_eq!(quiet.query, "rating <= 1");
        assert_eq!(quiet.exclude, "plays > 3");
    }
}
