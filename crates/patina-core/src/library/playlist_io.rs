//! Playlists in and out as M3U, M3U8 and PLS files, so they move between
//! PatinaPlayer and other players, or a phone and a desktop.
//!
//! Export writes extended M3U8: UTF-8, `#EXTINF` with the length and
//! "Artist - Title", then the path. Local songs are written as absolute
//! paths. Server songs are written as their `server://` paths, which
//! only PatinaPlayer understands; other players skip them.
//!
//! Import reads M3U, M3U8 and PLS. Each entry is matched to the library
//! by path first (relative paths are taken from the playlist's folder),
//! then by the last folders and file name, so a playlist made on another
//! machine with the music somewhere else still lines up. Entries that
//! match nothing are kept as missing songs if they're local paths, so
//! they turn up when the files do. Web addresses are skipped.

use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use super::{Library, Result, TrackRow, file_uri};

/// What an import made of a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Imported {
    pub id: i64,
    pub name: String,
    /// Entries in the file.
    pub entries: usize,
    /// Matched to songs in the library.
    pub matched: usize,
    /// Local paths that matched nothing, kept as missing songs.
    pub missing: usize,
    /// Web addresses, left out.
    pub skipped: usize,
}

/// One entry read from a playlist file.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub location: String,
    /// "Artist - Title" from #EXTINF or TitleN, if the file had one.
    pub title: Option<String>,
}

/// The kinds of file this reads, by extension.
pub fn is_playlist_file(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "m3u" | "m3u8" | "pls"))
}

/// Read a playlist file's entries in order.
pub fn read_entries(path: &Path) -> std::io::Result<Vec<Entry>> {
    let bytes = std::fs::read(path)?;
    // M3U is supposed to be Latin-1 and M3U8 UTF-8; in practice both are
    // usually UTF-8. Take UTF-8 when it is, Latin-1 when it isn't.
    let text = match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => e.into_bytes().iter().map(|&b| b as char).collect(),
    };
    let text = text.trim_start_matches('\u{feff}');
    let pls = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("pls"))
        || text
            .trim_start()
            .to_ascii_lowercase()
            .starts_with("[playlist]");
    Ok(if pls {
        parse_pls(text)
    } else {
        parse_m3u(text)
    })
}

pub fn parse_m3u(text: &str) -> Vec<Entry> {
    let mut out = Vec::new();
    let mut title = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(info) = line.strip_prefix("#EXTINF:") {
            // "#EXTINF:241,Nirvana - Blew"; the title is after the first
            // comma that isn't inside attributes.
            title = info
                .split_once(',')
                .map(|(_, t)| t.trim().to_string())
                .filter(|t| !t.is_empty());
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        out.push(Entry {
            location: line.to_string(),
            title: title.take(),
        });
    }
    out
}

pub fn parse_pls(text: &str) -> Vec<Entry> {
    let mut files: std::collections::BTreeMap<u32, Entry> = std::collections::BTreeMap::new();
    let mut titles: std::collections::HashMap<u32, String> = std::collections::HashMap::new();
    for line in text.lines() {
        let Some((k, v)) = line.trim().split_once('=') else {
            continue;
        };
        let k = k.trim().to_ascii_lowercase();
        let num = |pre: &str| k.strip_prefix(pre).and_then(|n| n.parse::<u32>().ok());
        if let Some(n) = num("file") {
            files.insert(
                n,
                Entry {
                    location: v.trim().to_string(),
                    title: None,
                },
            );
        } else if let Some(n) = num("title") {
            titles.insert(n, v.trim().to_string());
        }
    }
    files
        .into_iter()
        .map(|(n, mut e)| {
            e.title = titles.remove(&n).filter(|t| !t.is_empty());
            e
        })
        .collect()
}

fn is_url(s: &str) -> bool {
    let l = s.to_ascii_lowercase();
    [
        "http://", "https://", "mms://", "rtsp://", "rtmp://", "ftp://",
    ]
    .iter()
    .any(|p| l.starts_with(p))
}

/// "file:///home/me/x.flac" to a path, with %-escapes undone.
fn file_url_path(s: &str) -> Option<PathBuf> {
    let u = url::Url::parse(s).ok()?;
    if u.scheme() != "file" {
        return None;
    }
    u.to_file_path().ok()
}

/// Windows paths from another player ("D:\Music\x.flac") with their
/// separators turned round, so the tail can still match.
fn unwindows(s: &str) -> String {
    let s = s.replace('\\', "/");
    let b = s.as_bytes();
    if b.len() > 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
        s[2..].to_string()
    } else {
        s
    }
}

/// The location as a path, made absolute against the playlist's folder.
fn resolve(base: &Path, loc: &str) -> PathBuf {
    if let Some(p) = file_url_path(loc) {
        return p;
    }
    if loc.starts_with("server://") {
        return PathBuf::from(loc);
    }
    let loc = unwindows(loc);
    let p = PathBuf::from(&loc);
    if p.is_absolute() { p } else { base.join(p) }
}

/// Write a playlist's songs as M3U8. Returns how many were written.
pub fn write_m3u8(path: &Path, tracks: &[TrackRow]) -> std::io::Result<usize> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Write next to it, then move it over, so a failed write never leaves
    // half a playlist behind.
    let tmp = path.with_extension("m3u8.part");
    let mut f = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
    writeln!(f, "#EXTM3U")?;
    for t in tracks {
        let secs = t.duration.map(|d| d.as_secs() as i64).unwrap_or(-1);
        let title = match &t.artist {
            Some(a) => format!("{a} - {}", t.title),
            None => t.title.clone(),
        };
        // A line break in a tag would end the line early.
        let title = title.replace(['\r', '\n'], " ");
        writeln!(f, "#EXTINF:{secs},{title}")?;
        f.write_all(t.path.as_os_str().as_bytes())?;
        f.write_all(b"\n")?;
    }
    f.flush()?;
    drop(f);
    std::fs::rename(&tmp, path)?;
    Ok(tracks.len())
}

/// The name a playlist file suggests: its file name without the extension.
pub fn name_of(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "Imported".into())
}

/// A file name safe for a playlist name: no slashes or control characters.
pub fn file_name_for(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c == '/' || c.is_control() { '_' } else { c })
        .collect();
    let s = s.trim().trim_start_matches('.').to_string();
    if s.is_empty() { "playlist".into() } else { s }
}

impl Library {
    /// The song a playlist entry stands for: by exact path, then by the
    /// same last folders and file name somewhere in the library.
    pub fn match_entry(&self, base: &Path, e: &Entry) -> Result<Option<TrackRow>> {
        let p = resolve(base, &e.location);
        if let Some(t) = self.track_by_path(&p)? {
            return Ok(Some(t));
        }
        // "Artist/Album/01 Song.flac", then "Album/01 Song.flac", then the
        // file name alone, if only one song has it.
        let parts: Vec<&std::ffi::OsStr> = p.iter().collect();
        let Some(file) = parts.last() else {
            return Ok(None);
        };
        let file = file.to_string_lossy().into_owned();
        let mut st = self
            .conn
            .prepare_cached("SELECT path FROM tracks WHERE filename = ?1")?;
        let candidates: Vec<Vec<u8>> = st
            .query_map([&file], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        if candidates.is_empty() {
            return Ok(None);
        }
        for depth in [3usize, 2] {
            if parts.len() < depth {
                continue;
            }
            let mut tail = Vec::new();
            for (i, c) in parts[parts.len() - depth..].iter().enumerate() {
                if i > 0 {
                    tail.push(b'/');
                }
                tail.extend_from_slice(c.as_bytes());
            }
            let hits: Vec<&Vec<u8>> = candidates
                .iter()
                .filter(|c| {
                    c.ends_with(&tail)
                        && (c.len() == tail.len() || c[c.len() - tail.len() - 1] == b'/')
                })
                .collect();
            if hits.len() == 1 {
                return self.track_by_path(&super::bytes_path(hits[0].clone()));
            }
        }
        if candidates.len() == 1 {
            return self.track_by_path(&super::bytes_path(candidates[0].clone()));
        }
        Ok(None)
    }

    /// Read a playlist file into a new library playlist named `name`.
    pub fn import_playlist(&mut self, file: &Path, name: &str) -> Result<Imported> {
        let entries = read_entries(file)?;
        let base = file.parent().unwrap_or(Path::new("/")).to_path_buf();
        let mut uris = Vec::new();
        let (mut matched, mut missing, mut skipped) = (0, 0, 0);
        for e in &entries {
            if is_url(&e.location) {
                skipped += 1;
                continue;
            }
            match self.match_entry(&base, e)? {
                Some(t) => {
                    uris.push(t.uri());
                    matched += 1;
                }
                None => {
                    let p = resolve(&base, &e.location);
                    if p.starts_with("server://") {
                        skipped += 1;
                    } else {
                        uris.push(file_uri(&p));
                        missing += 1;
                    }
                }
            }
        }
        let id = self.create_playlist(name)?;
        self.append_uris(id, &uris)?;
        Ok(Imported {
            id,
            name: name.to_string(),
            entries: entries.len(),
            matched,
            missing,
            skipped,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_extended_m3u() {
        let e = parse_m3u(
            "#EXTM3U\n#EXTINF:241,Nirvana - Blew\nNirvana/Bleach/01 Blew.flac\n\n# a comment\nhttp://x/stream\n",
        );
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].title.as_deref(), Some("Nirvana - Blew"));
        assert_eq!(e[0].location, "Nirvana/Bleach/01 Blew.flac");
        assert_eq!(e[1].title, None);
    }

    #[test]
    fn reads_pls_in_number_order() {
        let e = parse_pls(
            "[playlist]\nFile2=/b.mp3\nTitle2=B\nFile1=/a.mp3\nNumberOfEntries=2\nVersion=2\n",
        );
        assert_eq!(
            e.iter().map(|x| x.location.as_str()).collect::<Vec<_>>(),
            ["/a.mp3", "/b.mp3"]
        );
        assert_eq!(e[1].title.as_deref(), Some("B"));
    }

    #[test]
    fn resolves_locations() {
        let base = Path::new("/music/lists");
        assert_eq!(
            resolve(base, "../A/x.flac"),
            PathBuf::from("/music/lists/../A/x.flac")
        );
        assert_eq!(resolve(base, "/abs/x.flac"), PathBuf::from("/abs/x.flac"));
        assert_eq!(
            resolve(base, "file:///abs/a%20b.flac"),
            PathBuf::from("/abs/a b.flac")
        );
        assert_eq!(
            resolve(base, "D:\\Music\\A\\x.flac"),
            PathBuf::from("/Music/A/x.flac")
        );
    }

    #[test]
    fn safe_file_names() {
        assert_eq!(file_name_for("AC/DC live"), "AC_DC live");
        assert_eq!(file_name_for(".."), "playlist");
    }
}
