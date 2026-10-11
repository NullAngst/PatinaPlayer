# PatinaPlayer

PatinaPlayer is a Linux music player for Subsonic servers and local folders, for people who want a minimal interface on top of a serious playback chain.

This is milestone 2 of the build plan: the audio engine from milestone 1, plus the library index and every library view in the terminal player. Radio and podcasts come next, then the GUI, then bit-perfect, Subsonic and the rest. What's here is built to the final shape, so later milestones add to it instead of replacing it.

## What works right now

**Library**
- Indexes your local folders into one SQLite database, so browsing is instant and works offline.
- Reads tags with lofty (ID3v2, Vorbis comments, MP4, APE), with FFmpeg as the fallback for anything lofty can't read. ReplayGain values are stored now so leveling can use them later.
- Only reads tags for new or changed files on startup, by size and modification time. A 3000-file test folder takes about a quarter second to index from scratch and about 10 ms to recheck.
- Watches the folders with inotify while it runs. Copy an album in and it shows up a couple of seconds later. Delete one and it's gone.
- If a folder root goes missing (an unmounted drive, say), its tracks are kept until it comes back instead of being wiped.

**Views**
- Folders: the real folder tree of every library root.
- Artists: by album artist (the default) or track artist, toggled with `t`.
- Albums: sorted by name, artist, year, recently added, most played or random.
- Songs: one flat list, sortable on seven columns, either direction.
- Genres: every genre with album and track counts, then its albums or its songs. "Blues Rock", "blues-rock" and "Blues_Rock" count as one genre.
- Playlists: create, rename, delete, add to from anywhere, reorder and remove items.
- Queue: what's playing and next, with reorder, remove, clear and save as playlist.
- Downloads: an empty placeholder until caching lands in milestone 7.
- Settings: shows what the config file currently says.
- Search: type-ahead over the index, artists, albums and songs at once. Every word has to match somewhere, so `nirv blea` finds Bleach.
- A queue panel (`q`) that shows what's coming up beside any view.

**Playback** (from milestone 1)
- Anything FFmpeg decodes: FLAC, ALAC, WAV, AIFF, APE, WavPack, MP3, AAC, Opus, Vorbis and the rest.
- Gapless playback. A test proves FLAC joins are sample-exact, and another proves MP3 padding is trimmed.
- Short fades on pause, resume, seek, next and stop. PipeWire output with ALSA fallback.
- Play counts. A track counts once you've actually listened to half of it or four minutes, whichever comes first. Seeking past the middle doesn't count.

A note on how albums are grouped. Tracks with an album artist tag group by album artist plus album name, wherever the files sit. Tracks without one group by album name plus folder, which keeps two unrelated "Greatest Hits" apart and keeps a compilation together even when every track has a different artist. Disc folders (`CD1`, `Disc 2`) are folded into their parent, so multi-disc albums come out as one.

## Prerequisites

- Rust, latest stable. I use rustup, so in my case that's `rustup update stable`.
- FFmpeg 5.1 or newer, with dev headers
- PipeWire and ALSA dev headers
- clang and pkg-config, since the FFmpeg and PipeWire bindings are generated at build time

SQLite is built into the binary, so there's nothing to install for it.

On Kubuntu (or any Ubuntu 24.04 based system):

```
sudo apt install pkg-config clang libavcodec-dev libavformat-dev libavutil-dev libswresample-dev libavfilter-dev libavdevice-dev libpipewire-0.3-dev libspa-0.2-dev libasound2-dev
```

On another distro the package names differ but the list is the same: FFmpeg, PipeWire, libspa, ALSA, clang. Fedora calls them `ffmpeg-free-devel` (or `ffmpeg-devel` from RPM Fusion), `pipewire-devel` and `alsa-lib-devel`, for example. You'll have to check yours.

The tests also need the `ffmpeg` command itself, since they generate their own audio files.

## Building

1. Extract the archive and go into it: `cd patinaplayer`
2. Build it: `cargo build --release`
3. Copy the binary somewhere on your PATH, like `cp target/release/patinaplayer-tui ~/.local/bin/`, since that's where user binaries usually live.
4. Optionally run the tests: `cargo test`

## First run

1. Run it once: `patinaplayer-tui`. That writes `~/.config/patinaplayer/config.toml`. If your system has a Music folder (`xdg-user-dir MUSIC`), it's already in the list.
2. Add any other folders: `patinaplayer-tui --add-folder /mnt/storage/music`. This edits the config and exits. Your comments in the file are kept.
3. Run it again. The first scan shows progress next to the folder in the sidebar, and the views fill in as it goes.

To take a folder back out: `patinaplayer-tui --remove-folder /mnt/storage/music`. Its tracks leave the index on the next start.

You can still play files directly without touching the library: `patinaplayer-tui song.flac ~/Downloads/SomeAlbum`. They go straight into the queue.

## Config

Everything lives in `~/.config/patinaplayer/config.toml`:

```toml
[library]
folders = ["~/Music", "/mnt/storage/music"]
watch = true             # pick up changes while running

[output]
backend = "auto"         # auto, pipewire, alsa or null
device = ""              # ALSA device or PipeWire node; empty is the default
buffer_ms = 200          # 50 to 1000

[playback]
volume = 80              # starting volume, 0 to 100
fade_ms = 100            # 0 turns fades off
```

A typo in a key name is an error, not a silent default, so you find out right away. Command line flags (`--backend`, `--device`, `--buffer-ms`, `--volume`, `--fade-ms`) override the file for one run.

Where everything else goes:

| What | Where |
|---|---|
| Settings | `~/.config/patinaplayer/config.toml` |
| Library index, playlists, play counts | `~/.local/share/patinaplayer/library.db` |

The index lives in `~/.local/share` on purpose, since cleaners like BleachBit wipe `~/.cache`, and nobody should lose their playlists to that.

## Keys

| Key | Action |
|---|---|
| Space | Play / pause |
| n / b | Next / back (back restarts the track if you're more than 3 seconds in) |
| Left / Right | Seek 5 s, Shift for 30 s |
| + / -, m | Volume, mute |
| s, r | Shuffle, cycle repeat |
| 1 to 8 | Folders, Artists, Albums, Songs, Genres, Playlists, Queue, Downloads |
| Tab | Switch between the sidebar and the list |
| / | Search |
| Enter | Open, or play from the selected track |
| Esc / Backspace | Back up a level |
| a / A | Add to the queue / play next |
| p | Add to a playlist |
| o / O | Sort Albums or Songs / reverse Songs |
| t | Artists: album or track artist. Genres: albums or songs |
| q | Queue panel |
| . | Jump to the playing track in Queue |
| F5 | Check the folders for changes |
| ? | Help |
| Ctrl+Q | Quit |

In the Queue: `d` remove, `J`/`K` move down or up, `S` save as a playlist, `C` clear. In Playlists: `N` new, `R` rename, `D` delete, and inside a playlist `d` and `J`/`K`.

Pressing Enter on a track plays the list it's in from that track, so Enter on track 3 of an album plays the album from track 3. Use `a` if you'd rather add to what's already queued.

The mouse works too: click to select, double-click to play or open, right-click to go back, and click or drag the seek and volume bars.

## How it's put together

```
crates/
  patina-core/          shared core, used by the TUI now and the GUI later
    src/audio/          decoder, gapless feeder, real-time renderer, outputs
    src/library/        SQLite index, tag reading, scanner, inotify service
    src/config.rs       config file and XDG paths
    src/player.rs       core thread: commands in, snapshots and events out
    src/queue.rs        queue on stable entry ids, shuffle, repeat
    examples/scan.rs    time an index of your own folders
    tests/              gapless, seek, library, watcher and player tests
  patina-tui/           the terminal frontend (ratatui)
```

The core owns all the state. Frontends send commands and draw what the core and the index report, which is what keeps the TUI and GUI identical later.

The library runs on its own thread with its own database connection. SQLite is in WAL mode, so the views keep reading while a scan writes. Paths are stored as raw bytes, since Linux file names don't have to be valid UTF-8.

The queue refers to entries by id, never by position. That's what lets you reorder or remove tracks while the decoder is already reading the next one, without anything getting confused.

To see how fast indexing is on your own collection: `cargo run --release -p patina-core --example scan -- ~/Music`. It uses a throwaway database, so your real index isn't touched.

## Known gaps in this milestone

- Cover art isn't drawn yet, in either frontend. The plan is to add it alongside the GUI in milestone 4, with kitty and sixel support in the TUI.
- There's no settings screen. Edit the config file and restart.
- When the next track has a different sample rate (44.1 kHz into 48 kHz, say), the output reopens at the new rate and there's a short gap. Gapless across rates needs the resampler, which is milestone 12.
- The TUI binary links FFmpeg, PipeWire and ALSA dynamically. The static TUI build is packaging work for milestone 14.
- 16-bit ALSA output is rounded, not dithered. TPDF dither lands with the output controls in milestone 12, and it only matters on the ALSA fallback with a device that refuses float and 32-bit.
- inotify needs one watch per folder. A very large library can run past `fs.inotify.max_user_watches`. If it does, you'll see an error saying so, and raising the limit with `sysctl` fixes it.

## License

GPL-3.0-or-later. See `LICENSE`.

Now PatinaPlayer is indexing your folders, keeping up with changes as you make them, and every library view is browsable from the terminal.
