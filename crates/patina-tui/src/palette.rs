//! The command palette (`:`): every action by name, so new ones don't
//! need a key of their own and old ones don't need remembering. Typing
//! narrows the list; Enter runs the action by sending its key, so the
//! palette does exactly what the key does, where the key works.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// One action in the palette.
pub struct Cmd {
    pub name: &'static str,
    /// The key, shown beside it, and sent when it's picked.
    pub keys: &'static str,
    code: KeyCode,
    mods: KeyModifiers,
    /// Works on the list, not the sidebar: the list gets the focus first.
    list: bool,
}

const fn c(name: &'static str, keys: &'static str, ch: char, list: bool) -> Cmd {
    Cmd {
        name,
        keys,
        code: KeyCode::Char(ch),
        mods: KeyModifiers::NONE,
        list,
    }
}

const fn ctrl(name: &'static str, keys: &'static str, ch: char) -> Cmd {
    Cmd {
        name,
        keys,
        code: KeyCode::Char(ch),
        mods: KeyModifiers::CONTROL,
        list: false,
    }
}

pub const COMMANDS: &[Cmd] = &[
    c("Play / pause", "c", 'c', false),
    c("Next song", "n", 'n', false),
    c("Back a song", "b", 'b', false),
    c("Mute", "m", 'm', false),
    c("Shuffle", "s", 's', false),
    c("Repeat: cycle", "r", 'r', false),
    c("Love the song playing", "l", 'l', false),
    c("Rate the song playing", "Y", 'Y', false),
    c("Queue panel", "q", 'q', false),
    c("Go to the song playing in the queue", ".", '.', true),
    c("Search the library", "/", '/', false),
    c("Keys: show them all", "?", '?', false),
    c("Folders", "1", '1', false),
    c("Artists", "2", '2', false),
    c("Albums", "3", '3', false),
    c("Songs", "4", '4', false),
    c("Genres", "5", '5', false),
    c("Playlists", "6", '6', false),
    c("Radio", "7", '7', false),
    c("Podcasts", "8", '8', false),
    c("Queue", "9", '9', false),
    c("Downloads", "0", '0', false),
    c("Recently played", "H", 'H', false),
    c("Play the whole view", "v", 'v', true),
    c("Play the whole view, shuffled", "V", 'V', true),
    c("Add the whole view to the queue", "e", 'e', true),
    c("Add the whole view to a playlist", "P", 'P', true),
    c("Add to the queue", "a", 'a', true),
    c("Play next", "A", 'A', true),
    c("Add to a playlist", "p", 'p', true),
    c("Go to album", "i", 'i', true),
    c("Go to artist", "I", 'I', true),
    c("Keep offline, or stop", "f", 'f', true),
    c("Keep the whole view offline, or stop", "F", 'F', true),
    c("Mix from the selection", "X", 'X', true),
    c("Rate the selection", "y", 'y', true),
    c("Star (Favorites)", "*", '*', true),
    c("Pick every row, or none", "U", 'U', true),
    c("Sort", "o", 'o', true),
    c("Reverse the sort", "O", 'O', true),
    c("Browse the next source", "B", 'B', true),
    c("Artist kind, or albums or songs", "t", 't', true),
    c("Sleep timer", "Z", 'Z', false),
    c("Podcast speed down", "[", '[', false),
    c("Podcast speed up", "]", ']', false),
    c("Silence trimming", "z", 'z', false),
    c("Import a playlist or smart playlist file", "<", '<', true),
    c("Export as M3U8", ">", '>', true),
    c("Export a smart playlist's rules", "}", '}', true),
    c("New playlist (Playlists)", "N", 'N', true),
    c("New smart playlist (Playlists)", "S", 'S', true),
    c("Rename (Playlists)", "R", 'R', true),
    c("Edit (smart playlist, station, server)", "E", 'E', true),
    c("Delete (Playlists, Radio)", "D", 'D', true),
    c("Test the station (Radio)", "T", 'T', true),
    c("Clear the queue (Queue)", "C", 'C', true),
    c("Load the server's queue (Queue)", "L", 'L', true),
    c("Save the mix playing (Queue)", "M", 'M', true),
    ctrl("Lyrics", "Ctrl+L", 'l'),
    ctrl("Now Playing", "Ctrl+N", 'n'),
    ctrl("Play on another device", "Ctrl+T", 't'),
    ctrl("Undo", "Ctrl+Z", 'z'),
    ctrl("Output device", "Ctrl+O", 'o'),
    ctrl("Send to the iPod", "Ctrl+P", 'p'),
    ctrl("EQ", "Ctrl+E", 'e'),
    ctrl("Amp", "Ctrl+A", 'a'),
    Cmd {
        name: "Check folders and feeds for changes",
        keys: "F5",
        code: KeyCode::F(5),
        mods: KeyModifiers::NONE,
        list: false,
    },
    ctrl("Quit", "Ctrl+Q", 'q'),
];

impl Cmd {
    /// The key it sends.
    pub fn event(&self) -> KeyEvent {
        KeyEvent::new(self.code, self.mods)
    }

    pub fn wants_list(&self) -> bool {
        self.list
    }
}

/// How well `name` matches what's typed: the letters in order, anywhere
/// (so "aqu" finds "Add to the queue"). Higher is better; None is no
/// match. Letters together, and at the start of words, count more.
pub fn score(name: &str, query: &str) -> Option<i32> {
    let q: Vec<char> = query
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    if q.is_empty() {
        return Some(0);
    }
    let n: Vec<char> = name.to_lowercase().chars().collect();
    let mut at = 0;
    let mut score = 0;
    let mut last: Option<usize> = None;
    for &want in &q {
        let found = (at..n.len()).find(|&i| n[i] == want)?;
        if last == Some(found.wrapping_sub(1)) {
            score += 5;
        }
        if found == 0 || !n[found - 1].is_alphanumeric() {
            score += 3;
        }
        score -= (found - at) as i32 / 4;
        last = Some(found);
        at = found + 1;
    }
    Some(score)
}

/// The commands matching what's typed, best first.
pub fn matches(query: &str) -> Vec<&'static Cmd> {
    let mut hits: Vec<(i32, usize, &'static Cmd)> = COMMANDS
        .iter()
        .enumerate()
        .filter_map(|(i, c)| score(c.name, query).map(|s| (s, i, c)))
        .collect();
    hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    hits.into_iter().map(|(_, _, c)| c).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_in_order_match() {
        assert!(score("Add to the queue", "aqu").is_some());
        assert!(score("Add to the queue", "qa").is_none());
        assert_eq!(score("Anything", ""), Some(0));
    }

    #[test]
    fn the_closest_comes_first() {
        let m = matches("lyrics");
        assert_eq!(m[0].name, "Lyrics");
        let m = matches("play on");
        assert_eq!(m[0].name, "Play on another device");
        assert!(matches("zzzz qqqq").is_empty());
    }
}
