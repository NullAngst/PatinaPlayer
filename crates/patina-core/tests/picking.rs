//! The additions of the 2026.11.12 update: picking several rows, the
//! listening history, smart playlists, playlist files in and out, the Now
//! Playing page, ListenBrainz and the iPod. A real indexed folder of
//! FFmpeg-made files and the null output, like browse.rs. Needs the
//! `ffmpeg` command on PATH; the iPod test also needs libgpod, and skips
//! without it.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command as Proc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use patina_core::browse::{Browser, Item, ListAction, Section, SettingsInfo, Startup};
use patina_core::config::Config;
use patina_core::library::Library;
use patina_core::library::smart;
use patina_core::{Backend, OutputConfig, Player, PlayerConfig};

fn make(path: &Path, tags: &[(&str, &str)]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut cmd = Proc::new("ffmpeg");
    cmd.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-y",
        "-f",
        "lavfi",
        "-i",
        "sine=f=440:r=44100:d=1",
    ]);
    for (k, v) in tags {
        cmd.arg("-metadata").arg(format!("{k}={v}"));
    }
    let st = cmd.arg(path).status().expect("ffmpeg must be installed");
    assert!(st.success(), "ffmpeg failed for {}", path.display());
}

struct Fixture {
    _dir: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
    config: PathBuf,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(dir.path()).unwrap();
    let root = base.join("Music");
    for n in 1..=3 {
        make(
            &root.join(format!("Nirvana/Bleach/0{n}.flac")),
            &[
                ("title", &format!("Bleach Song {n}")),
                ("artist", "Nirvana"),
                ("album_artist", "Nirvana"),
                ("album", "Bleach"),
                ("track", &n.to_string()),
                ("genre", "Grunge"),
                ("date", "1989"),
            ],
        );
    }
    make(
        &root.join("Other/Thing/01.mp3"),
        &[
            ("title", "Other Thing"),
            ("artist", "Someone"),
            ("album", "Thing"),
            ("genre", "Blues Rock; Folk"),
            ("date", "1971"),
        ],
    );
    let config = base.join("config.toml");
    std::fs::write(&config, "# test config\n").unwrap();
    Fixture {
        _dir: dir,
        base,
        root,
        config,
    }
}

fn browser(fx: &Fixture) -> Browser {
    let mut lib = Library::open_in_memory().unwrap();
    lib.index_folders(std::slice::from_ref(&fx.root)).unwrap();
    let player = Player::spawn(PlayerConfig {
        output: OutputConfig {
            backend: Backend::Null,
            ..Default::default()
        },
        fade_ms: 0,
        ..Default::default()
    });
    Browser::new(Startup {
        player,
        lib: Some(lib),
        service: None,
        pod: None,
        remote: None,
        cache: None,
        mixes: None,
        level: None,
        scrobble: None,
        listenbrainz: None,
        config: Config {
            library: patina_core::config::LibraryConfig {
                folders: vec![fx.root.clone()],
                watch: false,
            },
            ..Config::default()
        },
        settings_info: SettingsInfo {
            config_path: fx.config.clone(),
            ..Default::default()
        },
    })
}

fn items(b: &Browser) -> &[Item] {
    &b.level().expect("a level is open").items
}

fn titles(b: &Browser) -> Vec<String> {
    items(b)
        .iter()
        .filter_map(|i| match i {
            Item::Track(t) => Some(t.title.clone()),
            _ => None,
        })
        .collect()
}

fn index_of(b: &Browser, f: impl Fn(&Item) -> bool) -> usize {
    items(b).iter().position(f).expect("item not found")
}

fn notes(b: &mut Browser) -> String {
    b.take_notes()
        .into_iter()
        .map(|n| match n {
            patina_core::browse::Note::Info(s) | patina_core::browse::Note::Error(s) => s,
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

fn wait_queue(b: &Browser, what: &str, f: impl Fn(&patina_core::queue::Queue) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !f(&b.player.queue()) {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn queue_titles(b: &Browser) -> Vec<String> {
    let q = b.player.queue();
    (0..q.len())
        .filter_map(|i| q.at(i).and_then(|e| e.info.title.clone()))
        .collect()
}

#[test]
fn marked_songs_play_and_queue_in_list_order() {
    let fx = fixture();
    let mut b = browser(&fx);
    b.switch(Section::Songs);
    let s1 = index_of(
        &b,
        |i| matches!(i, Item::Track(t) if t.title == "Bleach Song 1"),
    );
    let s3 = index_of(
        &b,
        |i| matches!(i, Item::Track(t) if t.title == "Bleach Song 3"),
    );
    let other = index_of(
        &b,
        |i| matches!(i, Item::Track(t) if t.title == "Other Thing"),
    );
    // Picked out of order; they play in the list's order.
    b.toggle_mark(s3);
    b.toggle_mark(s1);
    assert_eq!(b.marked_count(), 2);
    assert!(b.is_marked(s1) && b.is_marked(s3) && !b.is_marked(other));
    b.play_selection();
    wait_queue(&b, "two songs", |q| q.len() == 2);
    assert_eq!(queue_titles(&b), ["Bleach Song 1", "Bleach Song 3"]);

    // A toggle takes one back out; a range picks everything between.
    b.toggle_mark(s1);
    assert_eq!(b.marked_count(), 1);
    b.clear_marks();
    b.toggle_mark(0);
    b.mark_range(3);
    assert_eq!(b.marked_count(), 4);
    b.add_selected_to_queue(false);
    wait_queue(&b, "four more", |q| q.len() == 6);

    // Marks stay on the same songs when the list is rebuilt.
    b.refresh_current();
    assert_eq!(b.marked_count(), 4);
}

#[test]
fn marked_rows_come_out_of_a_playlist_together_and_drag_reorders() {
    let fx = fixture();
    let mut b = browser(&fx);
    b.switch(Section::Albums);
    let bleach = index_of(&b, |i| matches!(i, Item::Album(a) if a.name == "Bleach"));
    b.select(bleach);
    let paths = b.playlist_paths_for_selection();
    b.create_playlist("Mine", &paths);
    b.switch(Section::Playlists);
    let p = index_of(&b, |i| matches!(i, Item::Playlist(p) if p.name == "Mine"));
    b.select(p);
    b.activate();
    assert_eq!(
        titles(&b),
        ["Bleach Song 1", "Bleach Song 2", "Bleach Song 3"]
    );

    // A drag moves one row to where it's dropped.
    b.move_playlist_item(2, 0);
    assert_eq!(
        titles(&b),
        ["Bleach Song 3", "Bleach Song 1", "Bleach Song 2"]
    );

    b.toggle_mark(0);
    b.toggle_mark(2);
    b.edit_playlist(true, 0);
    assert_eq!(titles(&b), ["Bleach Song 1"]);
    assert_eq!(b.marked_count(), 0);
}

#[test]
fn recently_played_lists_plays_newest_first_by_day() {
    let fx = fixture();
    let mut b = browser(&fx);
    let lib = b.lib.as_ref().unwrap();
    let one = fx.root.join("Nirvana/Bleach/01.flac");
    let other = fx.root.join("Other/Thing/01.mp3");
    lib.record_play(&one).unwrap();
    std::thread::sleep(Duration::from_millis(1100));
    lib.record_play(&other).unwrap();
    std::thread::sleep(Duration::from_millis(1100));
    lib.record_play(&one).unwrap();
    b.switch(Section::History);
    assert!(matches!(&items(&b)[0], Item::Header(h) if h == "Today"));
    assert_eq!(
        titles(&b),
        ["Bleach Song 1", "Other Thing", "Bleach Song 1"]
    );

    // Recently played as an album sort: the album played last first.
    b.prefs.album_sort = patina_core::library::AlbumSort::RecentlyPlayed;
    b.switch(Section::Albums);
    b.refresh_current();
    match &items(&b)[0] {
        Item::Album(a) => assert_eq!(a.name, "Bleach"),
        other => panic!("{other:?}"),
    }

    b.switch(Section::History);
    let clear = index_of(&b, |i| {
        matches!(
            i,
            Item::Action {
                action: ListAction::ClearHistory,
                ..
            }
        )
    });
    b.select(clear);
    b.activate();
    assert!(titles(&b).is_empty());
    // Play counts outlive the history.
    let lib = b.lib.as_ref().unwrap();
    assert_eq!(lib.track_by_path(&one).unwrap().unwrap().play_count, 2);
}

#[test]
fn smart_playlists_follow_the_library() {
    let fx = fixture();
    let mut b = browser(&fx);
    let check = b.check_smart("genre is grunge", "");
    assert!(check.ok, "{check:?}");
    assert_eq!(check.message, "Matches 3 tracks");
    let bad = b.check_smart("genre > 3", "");
    assert!(!bad.ok);
    assert!(
        !b.create_smart("Bad", "genre > 3", ""),
        "a bad query isn't saved"
    );

    assert!(b.create_smart("Grunge", "genre is grunge sort by title desc", ""));
    assert!(b.create_smart("Blues", "genre is \"blues rock\"", ""));
    assert!(b.create_smart("Old", "year < 1980", ""));
    assert!(b.create_smart("Rated", "rating >= 4", ""));
    b.switch(Section::Playlists);
    let g = index_of(&b, |i| matches!(i, Item::Smart(s) if s.name == "Grunge"));
    match &items(&b)[g] {
        Item::Smart(s) => assert_eq!(s.items, 3),
        _ => unreachable!(),
    }
    b.select(g);
    b.activate();
    assert_eq!(
        titles(&b),
        ["Bleach Song 3", "Bleach Song 2", "Bleach Song 1"]
    );
    assert!(b.can_play_view());

    let lib = b.lib.as_ref().unwrap();
    let sql = |q: &str| {
        let q = patina_core::library::smart::parse(q).unwrap();
        lib.query_tracks(&q, 0)
            .unwrap()
            .into_iter()
            .map(|t| t.title)
            .collect::<Vec<_>>()
    };
    assert_eq!(sql("genre is \"blues rock\""), ["Other Thing"]);
    assert_eq!(sql("genre contains blues"), ["Other Thing"]);
    assert_eq!(sql("year 1960-1979"), ["Other Thing"]);
    assert_eq!(sql("lossless").len(), 3);
    assert_eq!(sql("lossy"), ["Other Thing"]);
    assert_eq!(sql("format is mp3"), ["Other Thing"]);
    assert_eq!(sql("played never").len(), 4);
    assert_eq!(sql("added < 1d").len(), 4);
    assert_eq!(sql("added > 30d").len(), 0);
    assert_eq!(
        sql("not artist is nirvana or title starts with \"Bleach Song 2\"").len(),
        2
    );
    assert_eq!(sql("sort by random limit 2").len(), 2);
    assert_eq!(
        sql(&format!(
            "folder is \"{}\"",
            fx.root.join("Other").display()
        )),
        ["Other Thing"]
    );
    assert_eq!(sql("source is local").len(), 4);

    // Rate a song and the rating playlist picks it up on its own.
    let one = fx.root.join("Nirvana/Bleach/01.flac");
    lib.set_song_rating(&one, 5, false).unwrap();
    assert_eq!(sql("rating >= 4"), ["Bleach Song 1"]);
    b.refresh_section(Section::Playlists);
    b.switch(Section::Playlists);
    b.pop();
    let r = index_of(&b, |i| matches!(i, Item::Smart(s) if s.name == "Rated"));
    match &items(&b)[r] {
        Item::Smart(s) => assert_eq!(s.items, 1),
        _ => unreachable!(),
    }

    // Edit and delete.
    let row = match &items(&b)[r] {
        Item::Smart(s) => s.clone(),
        _ => unreachable!(),
    };
    assert!(b.update_smart(row.id, "Five stars", "rating = 5", ""));
    assert!(
        items(&b)
            .iter()
            .any(|i| matches!(i, Item::Smart(s) if s.name == "Five stars"))
    );
    b.delete_smart(row.id);
    assert!(
        !items(&b)
            .iter()
            .any(|i| matches!(i, Item::Smart(s) if s.id == row.id))
    );
}

/// A song has a row per genre, so "and" between two genres means a song
/// tagged with both, and "or" means either. When "and" finds nothing, the
/// live check says so and shows the "or" form.
#[test]
fn smart_genres_and_or() {
    let fx = fixture();
    let b = browser(&fx);
    let lib = b.lib.as_ref().unwrap();
    let n = |q: &str| {
        lib.query_count(&patina_core::library::smart::parse(q).unwrap())
            .unwrap()
    };
    // "Blues Rock; Folk" is one song with both.
    assert_eq!(n("genre is \"blues rock\" and genre is folk"), 1);
    assert_eq!(n("genre = grunge"), 3);
    // No song is both grunge and folk; either is all four.
    assert_eq!(n("genre = grunge and genre = folk"), 0);
    assert_eq!(n("genre = grunge or genre = folk"), 4);

    let c = b.check_smart("genre = grunge and genre = folk", "");
    assert!(c.ok);
    assert!(
        c.message
            .contains("use or: genre is grunge or genre is folk"),
        "{}",
        c.message
    );
    // Found something: no hint.
    let c = b.check_smart("genre is \"blues rock\" and genre is folk", "");
    assert_eq!(c.message, "Matches 1 track");
    // One-value fields get the hint too.
    let c = b.check_smart("artist is nirvana and artist is someone", "");
    assert!(
        c.message.contains("artist is nirvana or artist is someone"),
        "{}",
        c.message
    );
    // Different fields, nothing found: no hint, since that's a real "and".
    let c = b.check_smart("genre is folk and year < 1900", "");
    assert!(!c.message.contains("use or"), "{}", c.message);
}

/// Smart playlist rules look a little like SQL, so they get attacked like
/// SQL here. Every value typed in has to reach SQLite as a bound
/// parameter, never as part of the statement, so none of these can change
/// what the query does, and the library is the same after all of them.
#[test]
fn smart_playlists_cannot_inject_sql() {
    let fx = fixture();
    // A song whose tags are themselves an attack, so "is" has something
    // to match only if the value is taken as plain text.
    make(
        &fx.root.join("Evil/Tags/01.flac"),
        &[
            ("title", "Robert'); DROP TABLE tracks;--"),
            ("artist", "100% \"Pure\" O'Brien_"),
            ("album", "Tags"),
        ],
    );
    let b = browser(&fx);
    let lib = b.lib.as_ref().unwrap();
    let before = lib.track_count().unwrap();
    assert_eq!(before, 5);

    let run = |q: &str| -> Option<Vec<String>> {
        let q = patina_core::library::smart::parse(q).ok()?;
        let rows = lib
            .query_tracks(&q, 0)
            .unwrap_or_else(|e| panic!("valid rules made bad SQL: {e}"));
        Some(rows.into_iter().map(|t| t.title).collect())
    };

    // The attack strings match the song that has them, and only it.
    let evil = vec!["Robert'); DROP TABLE tracks;--".to_string()];
    assert_eq!(
        run("title is \"Robert'); DROP TABLE tracks;--\"").unwrap(),
        evil
    );
    assert_eq!(run("title contains \"DROP TABLE\"").unwrap(), evil);
    assert_eq!(run("artist contains \"O'Brien\"").unwrap(), evil);
    assert_eq!(run("artist contains '\"Pure\"'").unwrap(), evil);

    // LIKE's wildcards are taken literally: "%" and "_" only match songs
    // that have them, not every song.
    assert_eq!(run("artist contains \"%\"").unwrap(), evil);
    assert_eq!(run("artist ends \"_\"").unwrap(), evil);
    assert_eq!(run("title contains \"%%\"").unwrap().len(), 0);
    assert_eq!(run("title starts \"_\"").unwrap().len(), 0);

    // Classic payloads as values: they run, match nothing, change nothing.
    for q in [
        "title is \"x' OR '1'='1\"",
        "title is 'x\" OR \"1\"=\"1'",
        "title is \"x'; DELETE FROM tracks; --\"",
        "album contains \"') UNION SELECT * FROM smart_playlists --\"",
        "genre is \"a' OR 1=1 --\"",
        "genre contains \"%' OR '%'='\"",
        "folder is \"/'; DROP TABLE tracks; --\"",
        "folder contains \"'; DELETE FROM tracks; --\"",
        "source is \"x' OR '1'='1\"",
        "format is \"flac' OR 1=1 --\"",
    ] {
        assert_eq!(run(q).as_deref(), Some(&[][..]), "{q}");
    }

    // SQL in the places that aren't values isn't understood at all: the
    // fields, operators, sort keys and limit are from fixed lists.
    for q in [
        "title is x; DROP TABLE tracks",
        "year = 1 OR 1=1",
        "\"title) OR 1=1 --\" is x",
        "tracks.path is x",
        "title = x UNION SELECT 1",
        "sort by title; DROP TABLE tracks",
        "sort by (SELECT 1)",
        "limit 1; DROP TABLE tracks",
        "limit -1",
        "limit 1 OFFSET 0",
        "rating >= 4 --",
        "year >= (SELECT MAX(year) FROM tracks)",
        "length is 99999999999999999:00",
        "rate > 999999999999999999999k",
    ] {
        if let Some(rows) = run(q) {
            // A few of these are harmless rules once read as words; what
            // matters is they ran as rules and nothing else.
            assert!(rows.len() <= before as usize, "{q}");
        }
    }

    // Saving and reading back keeps the text exactly, attack and all.
    let id = lib
        .create_smart(
            "'); DROP TABLE smart_playlists;--",
            "title contains \"DROP TABLE\"",
            "artist is \"x'); DROP TABLE tracks;--\"",
        )
        .unwrap();
    let row = lib.smart_playlist(id).unwrap().unwrap();
    assert_eq!(row.name, "'); DROP TABLE smart_playlists;--");
    assert_eq!(row.exclude, "artist is \"x'); DROP TABLE tracks;--\"");
    assert_eq!(row.items, 1);
    assert_eq!(
        lib.smart_tracks(id).unwrap().len(),
        1,
        "the saved rules run the same way"
    );

    // Nested past the limit, too long, or too many rules: a message, not
    // a stack overflow or a crash.
    let deep = format!("{}title is x{}", "(".repeat(200), ")".repeat(200));
    assert!(patina_core::library::smart::parse(&deep).is_err());
    let nots = format!("{}title is x", "not ".repeat(500));
    assert!(patina_core::library::smart::parse(&nots).is_err());
    let huge = "(".repeat(1_000_000);
    assert!(patina_core::library::smart::parse(&huge).is_err());
    let many = vec!["year > 1"; 300].join(" or ");
    assert!(patina_core::library::smart::parse(&many).is_err());
    let ok = format!("{}title is x{}", "(".repeat(30), ")".repeat(30));
    assert!(run(&ok).is_some(), "30 deep is fine");

    // Random rules from every piece of the language and some junk: every
    // one either parses and runs as valid SQL, or is turned away with a
    // message. None panics, none makes SQLite reject the statement.
    let words = [
        "title",
        "artist",
        "albumartist",
        "album",
        "genre",
        "year",
        "rating",
        "albumrating",
        "plays",
        "played",
        "added",
        "length",
        "format",
        "rate",
        "bits",
        "lossless",
        "lossy",
        "unrated",
        "folder",
        "source",
        "is",
        "isnt",
        "contains",
        "starts",
        "ends",
        "=",
        "!=",
        "<",
        "<=",
        ">",
        ">=",
        "~",
        "and",
        "or",
        "not",
        "(",
        ")",
        "never",
        "sort",
        "by",
        "limit",
        "random",
        "desc",
        "asc",
        "4",
        "1990-1999",
        "30d",
        "2024-01",
        "4:30",
        "44.1k",
        "yes",
        "\"a b\"",
        "'it''s'",
        "\"%_\\\"",
        "local",
        "--",
        ";",
        "'",
        "\"",
        "DROP",
        "TABLE",
        "tracks",
        "SELECT",
        "*",
        "NULL",
        "0x41",
        "1e999",
        "-5",
    ];
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    for _ in 0..4000 {
        let mut q = String::new();
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let n = 1 + (seed % 12) as usize;
        for _ in 0..n {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            q.push_str(words[(seed % words.len() as u64) as usize]);
            q.push(' ');
        }
        if let Ok(parsed) = patina_core::library::smart::parse(&q) {
            lib.query_tracks(&parsed, 0)
                .unwrap_or_else(|e| panic!("{q:?} parsed but made bad SQL: {e}"));
            lib.query_count(&parsed)
                .unwrap_or_else(|e| panic!("{q:?} parsed but its count failed: {e}"));
        }
    }

    // And after all of it, the library is untouched.
    assert_eq!(lib.track_count().unwrap(), before);
    assert!(lib.smart_playlists().unwrap().iter().any(|s| s.id == id));
}

#[test]
fn playlists_go_out_as_m3u8_and_come_back() {
    let fx = fixture();
    let mut b = browser(&fx);
    b.switch(Section::Albums);
    let bleach = index_of(&b, |i| matches!(i, Item::Album(a) if a.name == "Bleach"));
    b.select(bleach);
    let paths = b.playlist_paths_for_selection();
    b.create_playlist("Bleach mix", &paths);
    b.switch(Section::Playlists);
    let p = index_of(
        &b,
        |i| matches!(i, Item::Playlist(p) if p.name == "Bleach mix"),
    );
    b.select(p);
    let (name, tracks) = b.export_subject().expect("a playlist to export");
    assert_eq!(name, "Bleach mix");
    assert_eq!(tracks.len(), 3);
    let out = fx.base.join("lists/Bleach mix.m3u8");
    b.export_to(&out);
    let text = std::fs::read_to_string(&out).unwrap();
    assert!(
        text.starts_with("#EXTM3U\n#EXTINF:1,Nirvana - Bleach Song 1\n"),
        "{text}"
    );
    assert!(text.contains(&fx.root.join("Nirvana/Bleach/02.flac").display().to_string()));
    assert_eq!(
        b.default_export_path("A/B"),
        fx.root.join("Playlists/A_B.m3u8")
    );

    // Back in, with relative paths, one moved (matched by its folders),
    // one that isn't anywhere and a web stream.
    let list = fx.root.join("lists/other.m3u");
    std::fs::create_dir_all(list.parent().unwrap()).unwrap();
    std::fs::write(
        &list,
        "#EXTM3U\n../Nirvana/Bleach/01.flac\n/somewhere/else/Other/Thing/01.mp3\n../Gone/x.flac\nhttp://radio.example/stream\n",
    )
    .unwrap();
    b.import_playlist_file(&list);
    let n = notes(&mut b);
    assert!(
        n.contains("Imported \"other\": 2 of 4 tracks matched"),
        "{n}"
    );
    assert!(n.contains("1 kept as missing"), "{n}");
    assert!(n.contains("1 web or server links left out"), "{n}");
    let p = index_of(&b, |i| matches!(i, Item::Playlist(p) if p.name == "other"));
    b.select(p);
    b.activate();
    assert_eq!(titles(&b), ["Bleach Song 1", "Other Thing"]);
    assert!(items(&b).iter().any(|i| matches!(i, Item::Missing { .. })));

    // PLS too.
    let pls = fx.base.join("x.pls");
    std::fs::write(
        &pls,
        format!(
            "[playlist]\nFile1={}\nTitle1=Bleach Song 3\nNumberOfEntries=1\n",
            fx.root.join("Nirvana/Bleach/03.flac").display()
        ),
    )
    .unwrap();
    b.import_playlist_file(&pls);
    assert!(notes(&mut b).contains("1 of 1 track matched"));
}

#[test]
fn now_playing_names_the_format_and_the_chain() {
    let fx = fixture();
    let mut b = browser(&fx);
    b.switch(Section::Songs);
    let s = index_of(
        &b,
        |i| matches!(i, Item::Track(t) if t.title == "Bleach Song 2"),
    );
    b.select(s);
    b.activate();
    let deadline = Instant::now() + Duration::from_secs(5);
    let snap = loop {
        let s = b.player.snapshot();
        if s.track.is_some() {
            break s;
        }
        assert!(Instant::now() < deadline, "nothing started");
        std::thread::sleep(Duration::from_millis(10));
    };
    let np = b.now_playing(&snap);
    assert_eq!(np.title, "Bleach Song 2");
    assert_eq!(np.artist, "Nirvana");
    assert!(np.format.starts_with("FLAC 44.1"), "{}", np.format);
    let stages: Vec<&str> = np.chain.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        stages,
        ["Leveling", "Amp stack", "EQ", "Channel tools", "Limiter"]
    );
    assert_eq!(np.chain[1].1, "off");
}

#[test]
fn listenbrainz_checks_the_token_and_sends_listens() {
    use common::{Response, serve};
    use serde_json::json;
    let got: Arc<Mutex<Vec<serde_json::Value>>> = Arc::default();
    let g = got.clone();
    let base = serve(Arc::new(move |r| {
        let auth = r.headers.get("authorization").cloned().unwrap_or_default();
        if auth != "Token good" {
            return Response {
                status: 401,
                ctype: "application/json".into(),
                body: json!({"code": 401, "error": "Invalid authorization token."})
                    .to_string()
                    .into_bytes(),
            };
        }
        match r.path.as_str() {
            "/1/validate-token" => {
                Response::json(json!({"valid": true, "user_name": "zack", "code": 200}))
            }
            "/1/submit-listens" => {
                g.lock()
                    .unwrap()
                    .push(serde_json::from_slice(&r.body).unwrap());
                Response::json(json!({"status": "ok"}))
            }
            _ => Response::not_found(),
        }
    }));
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("library.db");
    Library::open(&db).unwrap();
    let secrets = Arc::new(patina_core::secrets::Secrets::with(
        None,
        dir.path().join("secrets.toml"),
    ));
    let lb = patina_core::listenbrainz::ListenBrainz::start(&db, &base, "", secrets).unwrap();
    let wait = |what: &str, f: &dyn Fn() -> bool| {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !f() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    lb.set_token("bad");
    wait("refusal", &|| lb.status().problem.is_some());
    assert!(!lb.live());
    lb.set_token("good");
    wait("the account", &|| lb.status().user == "zack");
    assert!(lb.live());

    let play = patina_core::lastfm::Play {
        artist: "Nirvana".into(),
        track: "Blew".into(),
        album: Some("Bleach".into()),
        track_no: Some(1),
        duration: Some(175),
        timestamp: 1_700_000_000,
        ..Default::default()
    };
    lb.now_playing(play.clone());
    lb.listen(play);
    wait("two posts", &|| got.lock().unwrap().len() == 2);
    let posts = got.lock().unwrap().clone();
    assert_eq!(posts[0]["listen_type"], "playing_now");
    assert!(posts[0]["payload"][0].get("listened_at").is_none());
    assert_eq!(posts[1]["listen_type"], "single");
    let l = &posts[1]["payload"][0];
    assert_eq!(l["listened_at"], 1_700_000_000);
    assert_eq!(l["track_metadata"]["artist_name"], "Nirvana");
    assert_eq!(l["track_metadata"]["release_name"], "Bleach");
    assert_eq!(
        l["track_metadata"]["additional_info"]["duration_ms"],
        175_000
    );
    wait("the queue to drain", &|| lb.status().waiting == 0);
}

#[test]
fn an_ipod_takes_songs_converts_flac_and_gives_them_back() {
    let g = match patina_core::ipod::gpod::Gpod::load() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("skipping the iPod test: {e}");
            return;
        }
    };
    let fx = fixture();
    let b = browser(&fx);
    let mount = fx.base.join("IPOD");
    std::fs::create_dir_all(&mount).unwrap();
    // A 5th generation iPod (video, 30 GB), as libgpod knows it: the last
    // one before the database needed signing with the device's id.
    g.init(&mount, "MA002", "Zack's iPod").unwrap();
    assert_eq!(
        patina_core::ipod::find_in(
            &format!("/dev/sdz1 {} vfat rw 0 0\n", mount.display()),
            |p| p.join("iPod_Control/iTunes").is_dir()
        ),
        vec![(mount.clone(), "/dev/sdz1".to_string())]
    );

    let lib = b.lib.as_ref().unwrap();
    let songs: Vec<patina_core::ipod::SendSong> = lib
        .songs(patina_core::library::SongSort::Album, false)
        .unwrap()
        .into_iter()
        .map(|t| patina_core::ipod::SendSong {
            file: t.path.clone(),
            title: t.title.clone(),
            artist: t.artist.clone().unwrap_or_default(),
            album_artist: t.album_artist.clone().unwrap_or_default(),
            album: t.album.clone().unwrap_or_default(),
            genre: t.genre.clone().unwrap_or_default(),
            year: t.year.unwrap_or(0),
            track_no: t.track_no.unwrap_or(0) as i32,
            disc_no: 1,
            duration_ms: 1000,
            codec: t.codec.clone(),
            lossless: t.lossless,
            sample_rate: t.sample_rate,
            bits: t.bits,
            kbps: t.kbps.unwrap_or(0),
            art_from: t.path.clone(),
        })
        .collect();
    assert_eq!(songs.len(), 4);
    let stop = std::sync::atomic::AtomicBool::new(false);
    let send = |pl: Option<&str>| {
        let db = g.parse(&mount).unwrap();
        let msg = patina_core::ipod::send_into(
            &g,
            db,
            &mount,
            &songs,
            pl,
            patina_core::ipod::Target::Alac,
            &stop,
            |_, _, _| {},
        )
        .unwrap();
        g.write(db).unwrap();
        g.free(db);
        msg
    };
    let msg = send(Some("Road trip"));
    assert!(
        msg.starts_with("Sent 4 songs to the iPod, in \"Road trip\""),
        "{msg}"
    );
    // Again: nothing new goes, and nothing doubles.
    let msg = send(None);
    assert!(
        msg.contains("Sent 0 songs") && msg.contains("4 already on it"),
        "{msg}"
    );

    let c = patina_core::ipod::contents(&g, &mount).unwrap();
    assert_eq!(c.songs, 4);
    assert_eq!(c.playlists, vec![("Road trip".to_string(), 4)]);
    assert_eq!(c.albums.len(), 2);
    // The FLACs went over as Apple Lossless, the MP3 as it was.
    let files: Vec<String> = walk(&mount.join("iPod_Control/Music"))
        .iter()
        .filter_map(|p| p.extension().map(|e| e.to_string_lossy().to_lowercase()))
        .collect();
    assert_eq!(files.iter().filter(|e| *e == "m4a").count(), 3, "{files:?}");
    assert_eq!(files.iter().filter(|e| *e == "mp3").count(), 1, "{files:?}");

    let db = g.parse(&mount).unwrap();
    let msg = patina_core::ipod::remove_from(
        &g,
        db,
        &patina_core::ipod::Removal::Album {
            artist: "Nirvana".into(),
            album: "Bleach".into(),
        },
    )
    .unwrap();
    g.write(db).unwrap();
    g.free(db);
    assert!(msg.contains("3 songs"), "{msg}");
    let c = patina_core::ipod::contents(&g, &mount).unwrap();
    assert_eq!(c.songs, 1);
    assert_eq!(c.playlists, vec![("Road trip".to_string(), 1)]);
    assert_eq!(walk(&mount.join("iPod_Control/Music")).len(), 1);
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p);
            }
        }
    }
    out
}

#[test]
fn the_car_browses_playlists_and_plays_from_a_song() {
    use patina_core::browse::auto;
    let fx = fixture();
    let mut b = browser(&fx);
    b.switch(Section::Albums);
    let bleach = index_of(&b, |i| matches!(i, Item::Album(a) if a.name == "Bleach"));
    b.select(bleach);
    let paths = b.playlist_paths_for_selection();
    b.create_playlist("Drive", &paths);
    assert!(b.create_smart("Grunge", "genre is grunge", ""));
    b.toggle_favorite_selected();
    let lib = b.lib.as_ref().unwrap();

    let root: Vec<String> = auto::children(lib, "root")
        .into_iter()
        .map(|i| i.id)
        .collect();
    assert_eq!(
        root,
        [
            "shuffle",
            "favorites",
            "playlists",
            "smart",
            "history",
            "albums"
        ]
    );
    let lists = auto::children(lib, "playlists");
    let drive = lists
        .iter()
        .find(|i| i.title == "Drive")
        .expect("the playlist");
    assert!(drive.browsable && !drive.playable);
    let songs = auto::children(lib, &drive.id);
    assert_eq!(songs.len(), 3);
    assert!(songs.iter().all(|s| s.playable));
    assert_eq!(songs[1].title, "Bleach Song 2");
    match auto::resolve(lib, &songs[1].id) {
        auto::AutoPlay::Tracks { tracks, start } => {
            assert_eq!(start, 1);
            assert_eq!(tracks.len(), 3);
        }
        other => panic!("{other:?}"),
    }
    let favs = auto::children(lib, "favorites");
    assert_eq!(favs.len(), 1);
    assert_eq!(auto::children(lib, &favs[0].id).len(), 3);
    let smart = auto::children(lib, "smart");
    assert_eq!(auto::children(lib, &smart[0].id).len(), 3);

    // Playing it from the car plays the list from that song.
    let id = songs[2].id.clone();
    b.auto_play(&id);
    // The queue fills first and the current song is marked once it starts,
    // so wait for both.
    wait_queue(&b, "the playlist, from the third song", |q| {
        q.len() == 3 && q.current().and_then(|c| q.position(c)) == Some(2)
    });
}

/// The second box: songs to leave out. With the rules from the request:
/// the 60s pop songs, but not The Beatles, written either way.
#[test]
fn smart_playlists_leave_songs_out() {
    let fx = fixture();
    make(
        &fx.root.join("Beatles/Help/01.flac"),
        &[
            ("title", "Help!"),
            ("artist", "The Beatles"),
            ("album", "Help"),
            ("genre", "Pop"),
            ("date", "1965"),
        ],
    );
    // No artist tag at all: rules about the artist are unknown for it,
    // and that mustn't drop it from either side.
    make(
        &fx.root.join("Unknown/Tape/01.flac"),
        &[
            ("title", "Untagged Pop"),
            ("album", "Tape"),
            ("genre", "Pop"),
            ("date", "1965"),
        ],
    );
    let mut b = browser(&fx);
    let titles = |b: &Browser, q: &str, x: &str| -> Vec<String> {
        let lib = b.lib.as_ref().unwrap();
        let q = smart::parse_with(q, x).unwrap();
        let mut t: Vec<String> = lib
            .query_tracks(&q, 0)
            .unwrap()
            .into_iter()
            .map(|t| t.title)
            .collect();
        t.sort();
        t
    };

    // A range of years with "=".
    assert_eq!(
        titles(&b, "year = 1960-1969 and genre = pop", ""),
        ["Help!", "Untagged Pop"]
    );
    // The same, leaving out The Beatles however they're tagged. The song
    // with no artist stays.
    let leave_out = "artist = \"the beatles\" or artist = beatles";
    assert_eq!(
        titles(&b, "year = 1960-1969 and genre = pop", leave_out),
        ["Untagged Pop"]
    );
    // Added in the last 60 days, pop.
    assert_eq!(
        titles(&b, "added < 60d and genre = pop", leave_out),
        ["Untagged Pop"]
    );
    // An empty first box is every song, less the left out.
    assert_eq!(titles(&b, "", leave_out).len(), 5);

    // isnt and not keep songs without the tag, as the second box does.
    assert_eq!(titles(&b, "artist isnt \"the beatles\"", "").len(), 5);
    assert_eq!(titles(&b, "not (artist is \"the beatles\")", "").len(), 5);
    assert!(titles(&b, "artist isnt \"the beatles\"", "").contains(&"Untagged Pop".to_string()));

    // The check counts what the second box takes out, so a rule that
    // misses shows as 0.
    let c = b.check_smart("genre = pop", leave_out);
    assert!(c.ok);
    assert_eq!(c.message, "Matches 1 track, 1 left out");
    let c = b.check_smart("genre = pop", "artist = beetles");
    assert_eq!(c.message, "Matches 2 tracks, 0 left out");
    // A limit doesn't hide what was left out.
    let c = b.check_smart("genre = pop limit 1", leave_out);
    assert_eq!(c.message, "Matches 1 track, 1 left out");

    // Mistakes say which box they're in.
    let c = b.check_smart("genre = pop", "artist >");
    assert!(
        !c.ok && c.message.starts_with("Leave out: "),
        "{}",
        c.message
    );
    let c = b.check_smart("genre = pop", "artist = x sort by title");
    assert!(!c.ok && c.message.contains("first box"), "{}", c.message);
    assert!(!b.create_smart("Bad", "genre = pop", "limit 3"));

    // Saved, it keeps both boxes and plays what they match.
    assert!(b.create_smart("60s pop", "year = 1960-1969 and genre = pop", leave_out));
    let lib = b.lib.as_ref().unwrap();
    let row = lib
        .smart_playlists()
        .unwrap()
        .into_iter()
        .find(|s| s.name == "60s pop")
        .unwrap();
    assert_eq!(row.exclude, leave_out);
    assert_eq!(row.items, 1);
    let songs: Vec<String> = lib
        .smart_tracks(row.id)
        .unwrap()
        .into_iter()
        .map(|t| t.title)
        .collect();
    assert_eq!(songs, ["Untagged Pop"]);
    // Renaming keeps the second box.
    assert!(b.update_smart(row.id, "Sixties pop", &row.query, &row.exclude));
    let lib = b.lib.as_ref().unwrap();
    assert_eq!(
        lib.smart_playlist(row.id).unwrap().unwrap().exclude,
        leave_out
    );
}
