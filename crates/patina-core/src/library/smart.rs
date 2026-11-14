//! Smart playlists: a saved query over the library, run again every time
//! the playlist opens, so it's never out of date.
//!
//! The query is a line of text, the same in the TUI, the desktop and the
//! phone:
//!
//! ```text
//! rating >= 4 and genre is "blues rock"
//! played > 180d                      not played in six months (or never)
//! added < 30d sort by added          added in the last 30 days, newest first
//! artist contains nirvana and not album contains live
//! lossless and rate > 48k sort by random limit 100
//! ```
//!
//! Clauses are `field op value`. `and` is implied between clauses, `or`
//! and parentheses group them, `not` turns one round. A trailing
//! `sort by <key> [asc|desc]` and `limit <n>` shape the result.

use std::fmt;

pub mod form;

use rusqlite::params;
use rusqlite::types::Value;

use super::tags::normalize_genre;
use super::{Library, Result, TrackRow, now_secs, subtree_range};

/// The help line both frontends show next to the query box.
pub const HELP: &str = "Fields: title, artist, albumartist, album, genre, composer, grouping, category, comment, \
year, bpm, rating, albumrating, plays, played, added, modified, length, track, disc, format, rate, bits, bitrate, \
channels, size, lossless, folder, source. \
Example: rating >= 4 and genre is \"blues rock\" and played > 180d sort by random limit 100";

/// Examples, for an empty query box.
pub const EXAMPLES: &[(&str, &str)] = &[
    ("rating >= 4", "four and five star songs"),
    ("played > 180d", "not played in six months, or never"),
    (
        "added < 30d sort by added",
        "added in the last 30 days, newest first",
    ),
    (
        "genre is \"blues rock\" and year 1965-1975",
        "a genre in a span of years",
    ),
    (
        "plays = 0 and lossless",
        "lossless songs you haven't played yet",
    ),
    ("sort by random limit 50", "50 random songs"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Title,
    Artist,
    AlbumArtist,
    Album,
    Genre,
    Year,
    Rating,
    AlbumRating,
    Plays,
    Played,
    Added,
    Length,
    Format,
    Rate,
    Bits,
    Lossless,
    Folder,
    Source,
    Bpm,
    Composer,
    Grouping,
    Category,
    Comment,
    TrackNo,
    DiscNo,
    Bitrate,
    /// In MB.
    Size,
    Channels,
    /// When the file last changed on disk.
    Modified,
}

impl Field {
    fn parse(w: &str) -> Option<Self> {
        Some(match w {
            "title" | "name" | "song" => Field::Title,
            "artist" => Field::Artist,
            "albumartist" | "album_artist" | "album-artist" => Field::AlbumArtist,
            "album" => Field::Album,
            "genre" => Field::Genre,
            "year" => Field::Year,
            "rating" | "stars" => Field::Rating,
            "albumrating" | "album_rating" | "album-rating" => Field::AlbumRating,
            "plays" | "playcount" | "count" => Field::Plays,
            "played" | "lastplayed" | "last_played" => Field::Played,
            "added" => Field::Added,
            "length" | "duration" | "time" => Field::Length,
            "format" | "codec" => Field::Format,
            "rate" | "samplerate" | "sample_rate" => Field::Rate,
            "bits" | "depth" => Field::Bits,
            "lossless" => Field::Lossless,
            "folder" | "path" | "in" => Field::Folder,
            "source" | "server" => Field::Source,
            "bpm" | "tempo" => Field::Bpm,
            "composer" => Field::Composer,
            "grouping" | "group" => Field::Grouping,
            "category" => Field::Category,
            "comment" | "comments" => Field::Comment,
            "track" | "trackno" | "track_no" | "tracknumber" => Field::TrackNo,
            "disc" | "disk" | "discno" | "disc_no" | "discnumber" => Field::DiscNo,
            "bitrate" | "kbps" => Field::Bitrate,
            "size" | "filesize" => Field::Size,
            "channels" => Field::Channels,
            "modified" | "changed" | "mtime" => Field::Modified,
            _ => return None,
        })
    }

    fn kind(self) -> Kind {
        match self {
            Field::Title
            | Field::Artist
            | Field::AlbumArtist
            | Field::Album
            | Field::Format
            | Field::Composer
            | Field::Grouping
            | Field::Category
            | Field::Comment => Kind::Text,
            Field::Genre => Kind::Genre,
            Field::Year
            | Field::Rating
            | Field::AlbumRating
            | Field::Plays
            | Field::Length
            | Field::Rate
            | Field::Bits
            | Field::Bpm
            | Field::TrackNo
            | Field::DiscNo
            | Field::Bitrate
            | Field::Size
            | Field::Channels => Kind::Number,
            Field::Played | Field::Added | Field::Modified => Kind::Time,
            Field::Lossless => Kind::Bool,
            Field::Folder => Kind::Folder,
            Field::Source => Kind::Source,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Field::Title => "title",
            Field::Artist => "artist",
            Field::AlbumArtist => "albumartist",
            Field::Album => "album",
            Field::Genre => "genre",
            Field::Year => "year",
            Field::Rating => "rating",
            Field::AlbumRating => "albumrating",
            Field::Plays => "plays",
            Field::Played => "played",
            Field::Added => "added",
            Field::Length => "length",
            Field::Format => "format",
            Field::Rate => "rate",
            Field::Bits => "bits",
            Field::Lossless => "lossless",
            Field::Folder => "folder",
            Field::Source => "source",
            Field::Bpm => "bpm",
            Field::Composer => "composer",
            Field::Grouping => "grouping",
            Field::Category => "category",
            Field::Comment => "comment",
            Field::TrackNo => "track",
            Field::DiscNo => "disc",
            Field::Bitrate => "bitrate",
            Field::Size => "size",
            Field::Channels => "channels",
            Field::Modified => "modified",
        }
    }

    /// The SQL for the field's value, on a row of `tracks`.
    fn column(self) -> &'static str {
        match self {
            Field::Title => "COALESCE(title, filename)",
            Field::Artist => "COALESCE(artist, '')",
            Field::AlbumArtist => "COALESCE(album_artist, artist, '')",
            Field::Album => "COALESCE(album, '')",
            Field::Format => "COALESCE(codec, '')",
            Field::Year => "year",
            Field::Rating => {
                "COALESCE((SELECT stars FROM song_ratings sr WHERE sr.path = tracks.path), 0)"
            }
            Field::AlbumRating => {
                "COALESCE((SELECT stars FROM album_ratings ar WHERE ar.key = tracks.album_key), 0)"
            }
            Field::Plays => "play_count",
            Field::Played => "last_played",
            Field::Added => "added_at",
            Field::Length => "(duration_ms / 1000)",
            Field::Rate => "sample_rate",
            Field::Bits => "bits",
            Field::Lossless => "lossless",
            Field::Bpm => "bpm",
            Field::Composer => "COALESCE(composer, '')",
            Field::Grouping => "COALESCE(grouping, '')",
            Field::Category => "COALESCE(category, '')",
            Field::Comment => "COALESCE(comment, '')",
            Field::TrackNo => "track_no",
            Field::DiscNo => "disc_no",
            Field::Bitrate => "kbps",
            Field::Size => "(size / 1048576)",
            Field::Channels => "channels",
            // A file not looked at since an update (0) counts as unknown.
            Field::Modified => "NULLIF(mtime, 0)",
            Field::Genre | Field::Folder | Field::Source => "",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Text,
    Genre,
    Number,
    Time,
    Bool,
    Folder,
    Source,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Is,
    IsNot,
    Contains,
    Starts,
    Ends,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    And(Vec<Expr>),
    Or(Vec<Expr>),
    Not(Box<Expr>),
    Cond {
        field: Field,
        op: Op,
        value: String,
    },
    /// `played never`
    Never(Field),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Album,
    Title,
    Artist,
    Year,
    Added,
    Played,
    Plays,
    Rating,
    Length,
    Random,
}

impl SortKey {
    fn parse(w: &str) -> Option<Self> {
        Some(match w {
            "album" => SortKey::Album,
            "title" | "name" => SortKey::Title,
            "artist" => SortKey::Artist,
            "year" => SortKey::Year,
            "added" => SortKey::Added,
            "played" | "lastplayed" => SortKey::Played,
            "plays" | "playcount" => SortKey::Plays,
            "rating" | "stars" => SortKey::Rating,
            "length" | "duration" => SortKey::Length,
            "random" | "shuffle" => SortKey::Random,
            _ => return None,
        })
    }

    /// Newest, most and highest first, unless asked otherwise.
    fn desc_by_default(self) -> bool {
        matches!(
            self,
            SortKey::Added | SortKey::Played | SortKey::Plays | SortKey::Rating
        )
    }

    fn order_by(self, desc: bool) -> String {
        let d = if desc { " DESC" } else { "" };
        let album = "album_key, disc_no, track_no, filename";
        match self {
            SortKey::Album => {
                format!("COALESCE(album_artist, artist) COLLATE NOCASE{d}, year, {album}")
            }
            SortKey::Title => format!("COALESCE(title, filename) COLLATE NOCASE{d}"),
            SortKey::Artist => format!("artist IS NULL, artist COLLATE NOCASE{d}, year, {album}"),
            SortKey::Year => format!("year IS NULL, year{d}, {album}"),
            SortKey::Added => format!("added_at{d}, {album}"),
            SortKey::Played => format!("last_played IS NULL, last_played{d}"),
            SortKey::Plays => format!("play_count{d}, last_played DESC"),
            SortKey::Rating => format!("{}{d}, {album}", Field::Rating.column()),
            SortKey::Length => format!("duration_ms{d}"),
            // Shuffled in Rust, with a seed that holds still while the app
            // runs, so the list doesn't reshuffle under you.
            SortKey::Random => "id".into(),
        }
    }
}

/// How much a smart playlist holds at most: songs, or as many as fit in
/// a length of time or a size, taken in the playlist's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limit {
    Songs(u32),
    Minutes(u32),
    Hours(u32),
    Mb(u32),
    Gb(u32),
}

impl Limit {
    /// As the query writes it: "100", "90 minutes", "4 gb".
    pub fn text(self) -> String {
        match self {
            Limit::Songs(n) => n.to_string(),
            Limit::Minutes(n) => format!("{n} minutes"),
            Limit::Hours(n) => format!("{n} hours"),
            Limit::Mb(n) => format!("{n} mb"),
            Limit::Gb(n) => format!("{n} gb"),
        }
    }

    pub fn count(self) -> u32 {
        match self {
            Limit::Songs(n) | Limit::Minutes(n) | Limit::Hours(n) | Limit::Mb(n) | Limit::Gb(n) => {
                n
            }
        }
    }

    /// The most of a song's length (seconds) or size (bytes) it holds,
    /// for the kinds that aren't counted in songs.
    fn budget(self) -> Option<(bool, u64)> {
        match self {
            Limit::Songs(_) => None,
            Limit::Minutes(n) => Some((true, u64::from(n) * 60)),
            Limit::Hours(n) => Some((true, u64::from(n) * 3600)),
            Limit::Mb(n) => Some((false, u64::from(n) << 20)),
            Limit::Gb(n) => Some((false, u64::from(n) << 30)),
        }
    }

    fn unit(w: &str) -> Option<fn(u32) -> Limit> {
        Some(match w {
            "" | "songs" | "song" | "items" | "item" | "tracks" | "track" => Limit::Songs,
            "minutes" | "minute" | "min" | "mins" | "m" => Limit::Minutes,
            "hours" | "hour" | "h" | "hr" | "hrs" => Limit::Hours,
            "mb" | "megabytes" => Limit::Mb,
            "gb" | "gigabytes" => Limit::Gb,
            _ => return None,
        })
    }
}

/// A parsed query.
#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    pub expr: Option<Expr>,
    /// Songs to leave out, from the second box: any song these rules are
    /// true for goes, whatever the first box says.
    pub exclude: Option<Expr>,
    pub sort: Option<(SortKey, bool)>,
    pub limit: Option<Limit>,
}

impl Query {
    /// Why rules that match nothing might be asking for the wrong thing:
    /// two `is` rules on one field joined by `and`, like `genre is grunge
    /// and genre is post-grunge`. That asks for songs that are both at
    /// once, which reads like "either" but isn't. Says so, with the `or`
    /// that was likely meant. None when there's no such pair.
    pub fn both_hint(&self) -> Option<String> {
        fn find(e: &Expr) -> Option<(Field, String, String)> {
            match e {
                Expr::And(xs) => {
                    let is: Vec<(Field, &String)> = xs
                        .iter()
                        .filter_map(|x| match x {
                            Expr::Cond {
                                field,
                                op: Op::Is,
                                value,
                            } => Some((*field, value)),
                            _ => None,
                        })
                        .collect();
                    for (i, (f, a)) in is.iter().enumerate() {
                        if let Some((_, b)) = is[i + 1..]
                            .iter()
                            .find(|(g, b)| g == f && !b.eq_ignore_ascii_case(a))
                        {
                            return Some((*f, (*a).clone(), (*b).clone()));
                        }
                    }
                    xs.iter().find_map(find)
                }
                Expr::Or(xs) => xs.iter().find_map(find),
                Expr::Not(x) => find(x),
                _ => None,
            }
        }
        let (field, a, b) = find(self.expr.as_ref()?)?;
        let q = |v: &str| {
            if v.contains(char::is_whitespace) {
                format!("\"{v}\"")
            } else {
                v.to_string()
            }
        };
        let name = field.name();
        let either = format!("{name} is {} or {name} is {}", q(&a), q(&b));
        Some(if field == Field::Genre {
            format!(
                "\"and\" means a song tagged with both genres, and none are. For songs in either, use or: {either}"
            )
        } else {
            format!(
                "\"and\" means a song whose {name} is both at once, which can't be. For either, use or: {either}"
            )
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError(pub String);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ParseError {}

fn err<T>(msg: impl Into<String>) -> std::result::Result<T, ParseError> {
    Err(ParseError(msg.into()))
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Word(String),
    Str(String),
    Sym(&'static str),
    Open,
    Close,
}

fn lex(s: &str) -> std::result::Result<Vec<Tok>, ParseError> {
    let mut out = Vec::new();
    let mut it = s.chars().peekable();
    while let Some(&c) = it.peek() {
        if c.is_whitespace() || c == ',' {
            it.next();
        } else if c == '(' {
            it.next();
            out.push(Tok::Open);
        } else if c == ')' {
            it.next();
            out.push(Tok::Close);
        } else if c == '"' || c == '\'' || c == '\u{201c}' || c == '\u{2018}' {
            let close = match c {
                '\u{201c}' => '\u{201d}',
                '\u{2018}' => '\u{2019}',
                q => q,
            };
            it.next();
            let mut v = String::new();
            let mut closed = false;
            for ch in it.by_ref() {
                if ch == close || (close != c && ch == c) {
                    closed = true;
                    break;
                }
                v.push(ch);
            }
            if !closed {
                return err("A quote isn't closed");
            }
            out.push(Tok::Str(v));
        } else if "=!<>~^$".contains(c) {
            it.next();
            let two = it.peek().copied();
            let sym = match (c, two) {
                ('=', Some('=')) => {
                    it.next();
                    "="
                }
                ('!', Some('=')) | ('<', Some('>')) => {
                    it.next();
                    "!="
                }
                ('<', Some('=')) => {
                    it.next();
                    "<="
                }
                ('>', Some('=')) => {
                    it.next();
                    ">="
                }
                ('^', Some('=')) => {
                    it.next();
                    "^="
                }
                ('$', Some('=')) => {
                    it.next();
                    "$="
                }
                ('=', _) => "=",
                ('<', _) => "<",
                ('>', _) => ">",
                ('~', _) => "~",
                ('!', _) => "!",
                _ => return err(format!("\"{c}\" isn't something I know here")),
            };
            out.push(Tok::Sym(sym));
        } else {
            let mut w = String::new();
            while let Some(&ch) = it.peek() {
                if ch.is_whitespace() || "()=!<>~,\"".contains(ch) {
                    break;
                }
                // "^" and "$" only start an operator.
                if (ch == '^' || ch == '$') && !w.is_empty() {
                    break;
                }
                w.push(ch);
                it.next();
            }
            out.push(Tok::Word(w));
        }
    }
    Ok(out)
}

/// Limits on a query, so a pasted wall of text or ten thousand nested
/// parentheses gets a plain message instead of a stack overflow in the
/// parser or a query SQLite refuses. Far above anything a person types.
const MAX_CHARS: usize = 4000;
const MAX_DEPTH: usize = 32;
const MAX_CONDITIONS: usize = 200;

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
    /// How many "(" and "not" deep the parser is right now.
    depth: usize,
    /// Conditions read so far.
    conds: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn peek_word(&self) -> Option<String> {
        match self.peek() {
            Some(Tok::Word(w)) => Some(w.to_lowercase()),
            _ => None,
        }
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        self.pos += 1;
        t
    }

    fn eat_word(&mut self, w: &str) -> bool {
        if self.peek_word().as_deref() == Some(w) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    /// Where the condition part stops: the end, a ")" or "sort"/"limit".
    fn at_tail(&self) -> bool {
        match self.peek() {
            None | Some(Tok::Close) => true,
            Some(Tok::Word(w)) => matches!(w.to_lowercase().as_str(), "sort" | "limit"),
            _ => false,
        }
    }

    fn expr(&mut self) -> std::result::Result<Expr, ParseError> {
        let mut parts = vec![self.and_expr()?];
        while self.eat_word("or") {
            parts.push(self.and_expr()?);
        }
        Ok(if parts.len() == 1 {
            parts.pop().unwrap()
        } else {
            Expr::Or(parts)
        })
    }

    fn and_expr(&mut self) -> std::result::Result<Expr, ParseError> {
        let mut parts = vec![self.unary()?];
        loop {
            if self.eat_word("and") {
                parts.push(self.unary()?);
                continue;
            }
            if self.at_tail() || self.peek_word().as_deref() == Some("or") {
                break;
            }
            // Clauses side by side mean "and".
            parts.push(self.unary()?);
        }
        Ok(if parts.len() == 1 {
            parts.pop().unwrap()
        } else {
            Expr::And(parts)
        })
    }

    fn deeper(&mut self) -> std::result::Result<(), ParseError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return err(format!(
                "That's nested more than {MAX_DEPTH} deep. Take out some parentheses or nots"
            ));
        }
        Ok(())
    }

    fn unary(&mut self) -> std::result::Result<Expr, ParseError> {
        if self.eat_word("not") || matches!(self.peek(), Some(Tok::Sym("!"))) {
            if matches!(self.peek(), Some(Tok::Sym("!"))) {
                self.pos += 1;
            }
            self.deeper()?;
            let e = Expr::Not(Box::new(self.unary()?));
            self.depth -= 1;
            return Ok(e);
        }
        if matches!(self.peek(), Some(Tok::Open)) {
            self.pos += 1;
            self.deeper()?;
            let e = self.expr()?;
            if !matches!(self.next(), Some(Tok::Close)) {
                return err("A \"(\" isn't closed");
            }
            self.depth -= 1;
            return Ok(e);
        }
        self.conds += 1;
        if self.conds > MAX_CONDITIONS {
            return err(format!(
                "That's more than {MAX_CONDITIONS} rules. Split it into two smart playlists"
            ));
        }
        self.cond()
    }

    fn cond(&mut self) -> std::result::Result<Expr, ParseError> {
        let word = match self.next() {
            Some(Tok::Word(w)) => w.to_lowercase(),
            Some(Tok::Str(s)) => {
                return err(format!(
                    "\"{s}\" needs a field in front, like artist is \"{s}\""
                ));
            }
            Some(Tok::Close) => return err("There's a \")\" with no \"(\""),
            Some(Tok::Sym(s)) => return err(format!("\"{s}\" needs a field in front of it")),
            Some(Tok::Open) => unreachable!(),
            None => return err("The query stops halfway through a clause"),
        };
        // Words that stand alone.
        match word.as_str() {
            "lossy" => {
                return Ok(Expr::Cond {
                    field: Field::Lossless,
                    op: Op::Is,
                    value: "no".into(),
                });
            }
            "never" if self.eat_word("played") => return Ok(Expr::Never(Field::Played)),
            "unrated" => {
                return Ok(Expr::Cond {
                    field: Field::Rating,
                    op: Op::Is,
                    value: "0".into(),
                });
            }
            _ => {}
        }
        // "album artist" as two words.
        let word = if word == "album" && self.peek_word().as_deref() == Some("artist") {
            self.pos += 1;
            "albumartist".to_string()
        } else {
            word
        };
        let Some(field) = Field::parse(&word) else {
            return err(format!(
                "I don't know the field \"{word}\". {}",
                HELP.split(". Example").next().unwrap_or(HELP)
            ));
        };
        if field == Field::Lossless && !self.at_op() {
            return Ok(Expr::Cond {
                field,
                op: Op::Is,
                value: "yes".into(),
            });
        }
        if field == Field::Played && self.eat_word("never") {
            return Ok(Expr::Never(field));
        }
        let op = self.op(field)?;
        if field == Field::Played && op == Op::Is && self.eat_word("never") {
            return Ok(Expr::Never(field));
        }
        let value = match self.next() {
            Some(Tok::Word(w)) => w,
            Some(Tok::Str(s)) => s,
            _ => {
                return err(format!(
                    "{} {} needs a value after it",
                    field.name(),
                    op_name(op)
                ));
            }
        };
        let cond = Expr::Cond { field, op, value };
        check(&cond)?;
        Ok(cond)
    }

    fn at_op(&self) -> bool {
        match self.peek() {
            Some(Tok::Sym(_)) => true,
            Some(Tok::Word(w)) => matches!(
                w.to_lowercase().as_str(),
                "is" | "isnt" | "isn't" | "contains" | "has" | "starts" | "ends"
            ),
            _ => false,
        }
    }

    fn op(&mut self, field: Field) -> std::result::Result<Op, ParseError> {
        let op = match self.next() {
            Some(Tok::Sym(s)) => match s {
                "=" => Op::Is,
                "!=" => Op::IsNot,
                "<" => Op::Lt,
                "<=" => Op::Le,
                ">" => Op::Gt,
                ">=" => Op::Ge,
                "~" => Op::Contains,
                "^=" => Op::Starts,
                "$=" => Op::Ends,
                _ => return err(format!("\"{s}\" can't go after {}", field.name())),
            },
            Some(Tok::Word(w)) => match w.to_lowercase().as_str() {
                "is" => {
                    if self.eat_word("not") {
                        Op::IsNot
                    } else {
                        Op::Is
                    }
                }
                "isnt" | "isn't" => Op::IsNot,
                "contains" | "has" => Op::Contains,
                "starts" => {
                    self.eat_word("with");
                    Op::Starts
                }
                "ends" => {
                    self.eat_word("with");
                    Op::Ends
                }
                // "year 1990-1999", "rating 5": a value straight after.
                _ => {
                    self.pos -= 1;
                    Op::Is
                }
            },
            Some(Tok::Str(_)) => {
                self.pos -= 1;
                Op::Is
            }
            _ => return err(format!("{} needs something to compare with", field.name())),
        };
        Ok(op)
    }

    fn tail(&mut self, q: &mut Query) -> std::result::Result<(), ParseError> {
        loop {
            if self.eat_word("sort") || self.eat_word("order") {
                self.eat_word("by");
                let Some(w) = self.peek_word() else {
                    return err(
                        "sort by needs one of: album, title, artist, year, added, played, plays, rating, length, random",
                    );
                };
                self.pos += 1;
                let Some(key) = SortKey::parse(&w) else {
                    return err(format!(
                        "I can't sort by \"{w}\". Sort by album, title, artist, year, added, played, plays, rating, length or random"
                    ));
                };
                let desc = if self.eat_word("desc") || self.eat_word("descending") {
                    true
                } else if self.eat_word("asc") || self.eat_word("ascending") {
                    false
                } else {
                    key.desc_by_default()
                };
                q.sort = Some((key, desc));
            } else if self.eat_word("limit") || self.eat_word("first") {
                // "100", "90 minutes", "2 hours", "700 mb", or "4gb" as one word.
                let w = match self.next() {
                    Some(Tok::Word(w)) => w.to_lowercase(),
                    _ => String::new(),
                };
                let (num, mut unit) = split_unit(&w);
                let n = num.parse::<u32>().ok().filter(|n| *n > 0);
                let Some(n) = n else {
                    return err(
                        "limit needs a number: limit 100 (songs), limit 90 minutes, limit 2 hours, limit 700 mb, limit 4 gb",
                    );
                };
                let next = self.peek_word().unwrap_or_default();
                if unit.is_empty()
                    && !next.is_empty()
                    && !matches!(next.as_str(), "sort" | "order" | "limit" | "first")
                {
                    self.pos += 1;
                    unit = next.as_str();
                }
                let Some(make) = Limit::unit(unit) else {
                    return err(format!(
                        "limit takes songs, minutes, hours, mb or gb, not \"{unit}\""
                    ));
                };
                q.limit = Some(make(n));
            } else if self.peek().is_none() {
                return Ok(());
            } else {
                return err(match self.peek() {
                    Some(Tok::Close) => "There's a \")\" with no \"(\"".to_string(),
                    _ => "Only sort by and limit can come after them".to_string(),
                });
            }
        }
    }
}

fn op_name(op: Op) -> &'static str {
    match op {
        Op::Is => "is",
        Op::IsNot => "isn't",
        Op::Contains => "contains",
        Op::Starts => "starts with",
        Op::Ends => "ends with",
        Op::Lt => "<",
        Op::Le => "<=",
        Op::Gt => ">",
        Op::Ge => ">=",
    }
}

/// Reject a clause whose operator or value can't work for its field, at
/// parse time, so a bad query never gets saved.
fn check(e: &Expr) -> std::result::Result<(), ParseError> {
    let Expr::Cond { field, op, value } = e else {
        return Ok(());
    };
    let name = field.name();
    match field.kind() {
        Kind::Text | Kind::Genre => {
            if matches!(op, Op::Lt | Op::Le | Op::Gt | Op::Ge) {
                return err(format!(
                    "{name} is text: use is, isn't, contains, starts with or ends with"
                ));
            }
            if *field == Field::Genre && matches!(op, Op::Starts | Op::Ends) {
                return err("genre takes is, isn't or contains");
            }
        }
        Kind::Number => {
            if matches!(op, Op::Contains | Op::Starts | Op::Ends) {
                return err(format!("{name} is a number: use =, !=, <, <=, > or >="));
            }
            if range(value).is_some() && matches!(op, Op::Is | Op::IsNot) {
                return Ok(());
            }
            if number(*field, value).is_none() {
                return err(match field {
                    Field::Length => format!("\"{value}\" isn't a length. Try 4:30, 5m or 300s"),
                    Field::Rate => {
                        format!("\"{value}\" isn't a sample rate. Try 44100, 96k or 44.1k")
                    }
                    Field::Year => {
                        format!("\"{value}\" isn't a year. Try 1994, or 1990-1999 for a span")
                    }
                    _ => format!("\"{value}\" isn't a number"),
                });
            }
        }
        Kind::Time => {
            if !matches!(op, Op::Lt | Op::Le | Op::Gt | Op::Ge) {
                return err(format!(
                    "{name} takes < or > with an age or a date: {name} < 30d is in the last 30 days, \
                     {name} > 6mo is longer ago than six months, {name} > 2024-01-01 is after that day"
                ));
            }
            if age(value).is_none() && date(value).is_none() {
                return err(format!(
                    "\"{value}\" isn't an age or a date. Ages: 12h, 30d, 2w, 6mo, 1y. Dates: 2024-01-31"
                ));
            }
        }
        Kind::Bool => {
            if !matches!(op, Op::Is | Op::IsNot) || truth(value).is_none() {
                return err("lossless stands alone, or takes is yes or is no");
            }
        }
        Kind::Folder => {
            if !matches!(op, Op::Is | Op::IsNot | Op::Contains) {
                return err(
                    "folder takes is (the folder and everything under it), isn't or contains",
                );
            }
        }
        Kind::Source => {
            if !matches!(op, Op::Is | Op::IsNot) {
                return err("source takes is or isn't: source is local, source is Home");
            }
        }
    }
    Ok(())
}

/// Parse a query. An empty one matches everything.
pub fn parse(text: &str) -> std::result::Result<Query, ParseError> {
    if text.chars().count() > MAX_CHARS {
        return err(format!(
            "That's longer than {MAX_CHARS} characters. Shorten it, or split it into two smart playlists"
        ));
    }
    let toks = lex(text)?;
    let mut p = Parser {
        toks,
        pos: 0,
        depth: 0,
        conds: 0,
    };
    let mut q = Query {
        expr: None,
        exclude: None,
        sort: None,
        limit: None,
    };
    if !p.at_tail() {
        q.expr = Some(p.expr()?);
    }
    p.tail(&mut q)?;
    Ok(q)
}

/// Both boxes: the songs to take (`include`, with any sort and limit), and
/// the songs to leave out (`exclude`, rules only). Either can be empty;
/// an empty first box takes every song.
pub fn parse_with(include: &str, exclude: &str) -> std::result::Result<Query, ParseError> {
    let mut q = parse(include)?;
    if !exclude.trim().is_empty() {
        let x = parse(exclude).map_err(|e| ParseError(format!("Leave out: {e}")))?;
        if x.sort.is_some() || x.limit.is_some() {
            return err("Leave out: sort by and limit go in the first box");
        }
        q.exclude = x.expr;
    }
    Ok(q)
}

/// "4:30", "5m", "300s", "1h", "300" (seconds) as seconds; a sample rate
/// "96k", "44.1k", "96khz" as Hz; anything else as a plain number.
fn number(field: Field, v: &str) -> Option<i64> {
    let v = v.trim().to_lowercase();
    match field {
        Field::Length => {
            if let Some((m, s)) = v.split_once(':') {
                let (m, s) = (m.parse::<i64>().ok()?, s.parse::<i64>().ok()?);
                return m.checked_mul(60)?.checked_add(s);
            }
            let (n, unit) = split_unit(&v);
            let n: f64 = n.parse().ok()?;
            let mul = match unit {
                "" | "s" | "sec" | "secs" => 1.0,
                "m" | "min" | "mins" => 60.0,
                "h" | "hr" | "hrs" => 3600.0,
                _ => return None,
            };
            Some((n * mul).round() as i64)
        }
        Field::Rate => {
            let (n, unit) = split_unit(&v);
            let n: f64 = n.parse().ok()?;
            let mul = match unit {
                "" | "hz" => 1.0,
                "k" | "khz" => 1000.0,
                _ => return None,
            };
            Some((n * mul).round() as i64)
        }
        _ => v.parse().ok(),
    }
}

fn range(v: &str) -> Option<(i64, i64)> {
    let (a, b) = v.split_once('-')?;
    let (a, b) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
    Some(if a <= b { (a, b) } else { (b, a) })
}

fn split_unit(v: &str) -> (&str, &str) {
    let i = v
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(v.len());
    (&v[..i], v[i..].trim())
}

/// An age in seconds: "12h", "30d", "2w", "6mo", "1y". A bare number is days.
fn age(v: &str) -> Option<i64> {
    let v = v.trim().to_lowercase();
    let (n, unit) = split_unit(&v);
    let n: f64 = n.parse().ok()?;
    let day = 86_400.0;
    let mul = match unit {
        "h" | "hr" | "hrs" | "hour" | "hours" => 3600.0,
        "" | "d" | "day" | "days" => day,
        "w" | "wk" | "week" | "weeks" => 7.0 * day,
        // "m" is months here: nobody asks for songs added in the last
        // ten minutes.
        "m" | "mo" | "mon" | "month" | "months" => 30.0 * day,
        "y" | "yr" | "year" | "years" => 365.0 * day,
        _ => return None,
    };
    Some((n * mul).round() as i64)
}

/// A date, "2024-01-31" or "2024-01" or "2024", as the Unix time its day
/// starts, in local time.
fn date(v: &str) -> Option<i64> {
    use chrono::{Local, NaiveDate, TimeZone};
    let parts: Vec<&str> = v.split('-').collect();
    if parts
        .iter()
        .any(|p| p.is_empty() || !p.chars().all(|c| c.is_ascii_digit()))
    {
        return None;
    }
    let (y, m, d) = match parts.as_slice() {
        [y] if y.len() == 4 => (y.parse().ok()?, 1, 1),
        [y, m] => (y.parse().ok()?, m.parse().ok()?, 1),
        [y, m, d] => (y.parse().ok()?, m.parse().ok()?, d.parse().ok()?),
        _ => return None,
    };
    let day = NaiveDate::from_ymd_opt(y, m, d)?;
    Local
        .from_local_datetime(&day.and_hms_opt(0, 0, 0)?)
        .earliest()
        .map(|t| t.timestamp())
}

fn truth(v: &str) -> Option<bool> {
    match v.to_lowercase().as_str() {
        "yes" | "y" | "true" | "on" | "1" => Some(true),
        "no" | "n" | "false" | "off" | "0" => Some(false),
        _ => None,
    }
}

fn like_escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '%' | '_' | '\\') {
            o.push('\\');
        }
        o.push(c);
    }
    o
}

/// SQL for a query's conditions, with its parameters.
struct Sql {
    args: Vec<Value>,
    now: i64,
}

impl Sql {
    fn arg(&mut self, v: impl Into<Value>) -> String {
        self.args.push(v.into());
        format!("?{}", self.args.len())
    }

    fn expr(&mut self, e: &Expr) -> String {
        match e {
            Expr::And(v) => format!(
                "({})",
                v.iter()
                    .map(|x| self.expr(x))
                    .collect::<Vec<_>>()
                    .join(" AND ")
            ),
            Expr::Or(v) => format!(
                "({})",
                v.iter()
                    .map(|x| self.expr(x))
                    .collect::<Vec<_>>()
                    .join(" OR ")
            ),
            // A rule on a tag a song doesn't have is unknown to SQL, and
            // NOT unknown is still unknown, so the song would drop out of
            // both a rule and its opposite. Unknown counts as false here,
            // so "not (artist is x)" keeps songs with no artist.
            Expr::Not(x) => format!("NOT COALESCE({}, 0)", self.expr(x)),
            Expr::Never(_) => "(last_played IS NULL)".into(),
            Expr::Cond { field, op, value } => self.cond(*field, *op, value),
        }
    }

    fn cond(&mut self, field: Field, op: Op, value: &str) -> String {
        let col = field.column();
        match field.kind() {
            Kind::Text => {
                let (pat, eq) = match op {
                    Op::Is | Op::IsNot => (value.to_string(), true),
                    Op::Contains => (format!("%{}%", like_escape(value)), false),
                    Op::Starts => (format!("{}%", like_escape(value)), false),
                    Op::Ends => (format!("%{}", like_escape(value)), false),
                    _ => unreachable!("checked"),
                };
                let a = self.arg(pat);
                let test = if eq {
                    format!("{col} = {a} COLLATE NOCASE")
                } else {
                    format!("{col} LIKE {a} ESCAPE '\\'")
                };
                if op == Op::IsNot {
                    // No tag at all isn't the value either.
                    format!("NOT COALESCE(({test}), 0)")
                } else {
                    format!("({test})")
                }
            }
            Kind::Genre => {
                let norm = normalize_genre(value);
                let (test, a) = match op {
                    Op::Contains => (
                        "genre_norm LIKE",
                        self.arg(format!("%{}%", like_escape(&norm))),
                    ),
                    _ => ("genre_norm =", self.arg(norm)),
                };
                let esc = if op == Op::Contains {
                    " ESCAPE '\\'"
                } else {
                    ""
                };
                let exists = format!(
                    "EXISTS (SELECT 1 FROM track_genres g WHERE g.track_id = tracks.id AND g.{test} {a}{esc})"
                );
                if op == Op::IsNot {
                    format!("NOT {exists}")
                } else {
                    exists
                }
            }
            Kind::Number => {
                if let Some((lo, hi)) = range(value)
                    && matches!(op, Op::Is | Op::IsNot)
                {
                    let (a, b) = (self.arg(lo), self.arg(hi));
                    let not = if op == Op::IsNot { "NOT " } else { "" };
                    return format!("({col} IS NOT NULL AND {not}({col} BETWEEN {a} AND {b}))");
                }
                let n = number(field, value).unwrap_or(0);
                let a = self.arg(n);
                let sym = match op {
                    Op::Is => "=",
                    Op::IsNot => "!=",
                    Op::Lt => "<",
                    Op::Le => "<=",
                    Op::Gt => ">",
                    Op::Ge => ">=",
                    _ => unreachable!("checked"),
                };
                // A song with no year has no year below 2000 either.
                format!("({col} IS NOT NULL AND {col} {sym} {a})")
            }
            Kind::Time => {
                if let Some(secs) = age(value) {
                    let a = self.arg(self.now - secs);
                    // Younger than the age: after that time. Older than it:
                    // before it, and never for a song never played.
                    match op {
                        Op::Lt | Op::Le => format!("({col} IS NOT NULL AND {col} >= {a})"),
                        _ => format!("({col} IS NULL OR {col} < {a})"),
                    }
                } else {
                    let t = date(value).unwrap_or(0);
                    let a = self.arg(t);
                    let sym = match op {
                        Op::Lt => "<",
                        Op::Le => "<",
                        Op::Gt | Op::Ge => ">=",
                        _ => unreachable!("checked"),
                    };
                    // "> 2024-01-01" is from that day on; "<= 2024-01-01"
                    // takes the day in too.
                    if op == Op::Le {
                        let b = self.arg(t + 86_400);
                        let _ = a;
                        return format!("({col} IS NOT NULL AND {col} < {b})");
                    }
                    format!("({col} IS NOT NULL AND {col} {sym} {a})")
                }
            }
            Kind::Bool => {
                let want = truth(value).unwrap_or(true) != (op == Op::IsNot);
                format!("({col} = {})", i64::from(want))
            }
            Kind::Folder => {
                let p = crate::config::expand_tilde(value);
                let test = if op == Op::Contains {
                    // Bytes, since paths are stored as their raw bytes.
                    let a = self.arg(Value::Blob(
                        format!("%{}%", like_escape(&p.to_string_lossy())).into_bytes(),
                    ));
                    format!("(CAST(path AS TEXT) LIKE CAST({a} AS TEXT) ESCAPE '\\')")
                } else {
                    let (lo, hi) = subtree_range(&p);
                    let (a, b) = (self.arg(lo), self.arg(hi));
                    format!("(path >= {a} AND path < {b})")
                };
                if op == Op::IsNot {
                    format!("NOT {test}")
                } else {
                    test
                }
            }
            Kind::Source => {
                let prefix = b"server://".to_vec();
                let test = if value.eq_ignore_ascii_case("local") {
                    let a = self.arg(Value::Blob(prefix.clone()));
                    format!("(substr(path, 1, {}) != {a})", prefix.len())
                } else {
                    let (lo, hi) = subtree_range(&crate::remote::root(value));
                    let (a, b) = (self.arg(lo), self.arg(hi));
                    format!("(path >= {a} AND path < {b})")
                };
                if op == Op::IsNot {
                    format!("NOT {test}")
                } else {
                    test
                }
            }
        }
    }
}

/// A smart playlist, as Playlists lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct SmartRow {
    pub id: i64,
    pub name: String,
    pub query: String,
    /// The songs-to-leave-out rules, empty when there are none.
    pub exclude: String,
    /// Songs it matches right now.
    pub items: i64,
}

/// The seed for random order, fixed while the app runs.
fn seed() -> u64 {
    use std::sync::OnceLock;
    static SEED: OnceLock<u64> = OnceLock::new();
    *SEED.get_or_init(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9e37_79b9_7f4a_7c15)
            | 1
    })
}

fn shuffle<T>(v: &mut [T], mut s: u64) {
    for i in (1..v.len()).rev() {
        // xorshift64
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        v.swap(i, (s % (i as u64 + 1)) as usize);
    }
}

/// The WHERE for both boxes: the first box's rules, and none of the
/// second's. A song the second box's rules are unknown for (a tag it
/// doesn't have) isn't left out.
fn where_clause(sql: &mut Sql, q: &Query) -> String {
    let mut parts = Vec::new();
    if let Some(e) = &q.expr {
        parts.push(sql.expr(e));
    }
    if let Some(x) = &q.exclude {
        parts.push(format!("NOT COALESCE({}, 0)", sql.expr(x)));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", parts.join(" AND "))
    }
}

impl SmartRow {
    /// Both boxes on one line, for a list: "genre is pop · leaving out
    /// artist is beatles".
    pub fn rules(&self) -> String {
        if self.exclude.trim().is_empty() {
            self.query.clone()
        } else if self.query.trim().is_empty() {
            format!("everything · leaving out {}", self.exclude)
        } else {
            format!("{} · leaving out {}", self.query, self.exclude)
        }
    }
}

impl Library {
    fn smart_sql(&self, q: &Query) -> (String, Vec<Value>) {
        let mut sql = Sql {
            args: Vec::new(),
            now: now_secs(),
        };
        let mut clause = where_clause(&mut sql, q);
        let (key, desc) = q.sort.unwrap_or((SortKey::Album, false));
        clause.push_str(&format!(" ORDER BY {}", key.order_by(desc)));
        if key != SortKey::Random
            && let Some(Limit::Songs(n)) = q.limit
        {
            clause.push_str(&format!(" LIMIT {n}"));
        }
        (clause, sql.args)
    }

    /// The songs a query matches, in its order.
    pub fn query_tracks(&self, q: &Query, salt: i64) -> Result<Vec<TrackRow>> {
        let (clause, args) = self.smart_sql(q);
        let mut rows = self.tracks_where(&clause, rusqlite::params_from_iter(args))?;
        if q.sort.is_some_and(|(k, _)| k == SortKey::Random) {
            shuffle(
                &mut rows,
                seed() ^ (salt as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15),
            );
            if let Some(Limit::Songs(n)) = q.limit {
                rows.truncate(n as usize);
            }
        }
        // As many as fit in the time or size, in the playlist's order.
        if let Some((by_time, budget)) = q.limit.and_then(Limit::budget) {
            let mut used = 0u64;
            let mut keep = 0;
            for r in &rows {
                let cost = if by_time {
                    r.duration.map_or(0, |d| d.as_secs())
                } else {
                    r.size
                };
                if used + cost > budget {
                    break;
                }
                used += cost;
                keep += 1;
            }
            rows.truncate(keep);
        }
        Ok(rows)
    }

    /// How many songs a query matches, with its limit.
    pub fn query_count(&self, q: &Query) -> Result<i64> {
        let mut sql = Sql {
            args: Vec::new(),
            now: now_secs(),
        };
        let clause = where_clause(&mut sql, q);
        let n: i64 = self.conn.query_row(
            &format!("SELECT COUNT(*) FROM tracks {clause}"),
            rusqlite::params_from_iter(sql.args),
            |r| r.get(0),
        )?;
        Ok(match q.limit {
            Some(Limit::Songs(l)) => n.min(l as i64),
            // Time and size: only counting them out says how many fit.
            Some(_) => self.query_tracks(q, 0)?.len() as i64,
            None => n,
        })
    }

    pub fn smart_playlists(&self) -> Result<Vec<SmartRow>> {
        let mut st = self.conn.prepare_cached(
            "SELECT id, name, query, exclude FROM smart_playlists ORDER BY name COLLATE NOCASE, id",
        )?;
        let rows: Vec<(i64, String, String, String)> = st
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let mut out = Vec::with_capacity(rows.len());
        for (id, name, query, exclude) in rows {
            let items = match parse_with(&query, &exclude) {
                Ok(q) => self.query_count(&q)?,
                Err(_) => 0,
            };
            out.push(SmartRow {
                id,
                name,
                query,
                exclude,
                items,
            });
        }
        Ok(out)
    }

    pub fn smart_playlist(&self, id: i64) -> Result<Option<SmartRow>> {
        Ok(self.smart_playlists()?.into_iter().find(|s| s.id == id))
    }

    /// The songs a saved smart playlist matches right now.
    pub fn smart_tracks(&self, id: i64) -> Result<Vec<TrackRow>> {
        let Some((text, exclude)) = self
            .conn
            .query_row(
                "SELECT query, exclude FROM smart_playlists WHERE id = ?1",
                [id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .ok()
        else {
            return Ok(Vec::new());
        };
        match parse_with(&text, &exclude) {
            Ok(q) => self.query_tracks(&q, id),
            Err(_) => Ok(Vec::new()),
        }
    }

    /// Save a new smart playlist: its rules, and the rules for songs to
    /// leave out (empty for none). Both are checked first; a bad one comes
    /// back as the error, and nothing is saved.
    pub fn create_smart(
        &self,
        name: &str,
        query: &str,
        exclude: &str,
    ) -> std::result::Result<i64, SmartError> {
        parse_with(query, exclude)?;
        let now = now_secs();
        self.conn.execute(
            "INSERT INTO smart_playlists (name, query, exclude, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?4)",
            params![name, query.trim(), exclude.trim(), now],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn update_smart(
        &self,
        id: i64,
        name: &str,
        query: &str,
        exclude: &str,
    ) -> std::result::Result<(), SmartError> {
        parse_with(query, exclude)?;
        self.conn.execute(
            "UPDATE smart_playlists SET name = ?2, query = ?3, exclude = ?4, updated_at = ?5 \
             WHERE id = ?1",
            params![id, name, query.trim(), exclude.trim(), now_secs()],
        )?;
        Ok(())
    }

    pub fn delete_smart(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM smart_playlists WHERE id = ?1", [id])?;
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SmartError {
    #[error("{0}")]
    Query(#[from] ParseError),
    #[error("library database: {0}")]
    Db(#[from] rusqlite::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_examples() {
        for (q, _) in EXAMPLES {
            parse(q).unwrap_or_else(|e| panic!("{q}: {e}"));
        }
        parse("").unwrap();
        parse("album artist is \"The Band\" or (artist ~ nirvana and not album ~ live)").unwrap();
        parse("played never").unwrap();
        parse("never played").unwrap();
        parse("lossy sort by plays limit 5").unwrap();
        parse("length > 4:30 rate >= 96k bits = 24").unwrap();
        parse("bpm 120-130 and composer contains bach and grouping is \"Side A\"").unwrap();
        parse("category is jazz or comment contains live").unwrap();
        parse("track <= 3 and disc = 1 and bitrate >= 256 and size < 20 and channels = 2").unwrap();
        parse("modified < 30d").unwrap();
    }

    #[test]
    fn and_binds_tighter_than_or() {
        let q = parse("rating 5 or rating 4 and plays 0").unwrap();
        assert!(matches!(q.expr, Some(Expr::Or(ref v)) if v.len() == 2));
    }

    #[test]
    fn sort_and_limit() {
        let q = parse("sort by added limit 20").unwrap();
        assert_eq!(q.expr, None);
        assert_eq!(q.sort, Some((SortKey::Added, true)));
        assert_eq!(q.limit, Some(Limit::Songs(20)));
        for (text, want) in [
            ("limit 90 minutes", Limit::Minutes(90)),
            ("limit 2 hours", Limit::Hours(2)),
            ("limit 700 MB", Limit::Mb(700)),
            ("limit 4gb", Limit::Gb(4)),
            ("limit 25 songs", Limit::Songs(25)),
        ] {
            assert_eq!(parse(text).unwrap().limit, Some(want), "{text}");
            assert_eq!(
                parse(&format!("limit {}", want.text())).unwrap().limit,
                Some(want)
            );
        }
        let q = parse("sort by year asc").unwrap();
        assert_eq!(q.sort, Some((SortKey::Year, false)));
    }

    #[test]
    fn bad_queries_say_why() {
        for (q, has) in [
            ("ratting > 3", "don't know the field"),
            ("rating contains 4", "is a number"),
            ("artist > abc", "is text"),
            ("added is 30d", "takes < or >"),
            ("added < soon", "isn't an age"),
            ("artist is \"Nirvana", "isn't closed"),
            ("(rating 5", "isn't closed"),
            ("rating 5)", "no \"(\""),
            ("sort by colour", "can't sort by"),
            ("limit lots", "limit needs"),
            ("limit 5 fortnights", "limit takes"),
            ("length > forever", "isn't a length"),
        ] {
            let e = parse(q).expect_err(q);
            assert!(e.0.contains(has), "{q}: {e}");
        }
    }

    #[test]
    fn units() {
        assert_eq!(number(Field::Length, "4:30"), Some(270));
        assert_eq!(number(Field::Length, "5m"), Some(300));
        assert_eq!(number(Field::Rate, "44.1k"), Some(44100));
        assert_eq!(age("2w"), Some(14 * 86_400));
        assert_eq!(age("6mo"), Some(180 * 86_400));
        assert_eq!(range("1999-1990"), Some((1990, 1999)));
        assert!(date("2024-01-31").is_some());
        assert!(date("2024-13-01").is_none());
    }
}
