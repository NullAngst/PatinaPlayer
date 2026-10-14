# PatinaPlayer

PatinaPlayer is a Linux music player for Subsonic servers and local folders, for people who want a minimal interface on top of a serious playback chain.

This is milestone 5.5 of the build plan: everything from milestones 1 to 5 (the audio engine, the library, radio, podcasts, both apps, cover art, listening stats, the AppImage, bit-perfect output and an editable Settings view), plus whole views and the queue added to playlists, Add to queue next to Play and Shuffle, and two kawaii themes beside the default look. There are two programs on one core. `patinaplayer` is the desktop app and `patinaplayer-tui` runs in a terminal. Both show the same views, play through the same engine, and read and write the same library, playlists and config. The Subsonic client comes next, then the cache and the rest. What's here is built to the final shape, so later milestones add to it instead of replacing it.

## What works right now

**Desktop app**
- A Slint window in the dark verdigris look from the mockup (or one of the kawaii themes, below): sidebar of sections and sources, a header with back, forward and search, the open view, an optional queue panel, and the bottom bar with art, transport, seek, format badge, output and volume.
- Albums, and an artist's or a genre's albums, show as a cover grid. An album opens with a header showing its cover, year, track count and length, and buttons to play, shuffle, queue or add it to a playlist.
- Right-click any row for everything you can do with it: play, play next, add to queue, add to a playlist, and per kind edit, test, move or delete stations, unsubscribe, download, mark played, rename or delete playlists.
- Dialogs for new and edited stations, subscribing, new and renamed playlists, picking a playlist, and confirming deletes.
- The keyboard works the same as the TUI for almost everything. `?` lists the keys.
- The podcast speed chip in the bottom bar cycles 1.0x, 1.2x, 1.5x and 2.0x on click.
- On a narrow window the queue panel folds away and the bottom bar drops the format badge and output chip, so nothing gets cut off.
- Inter, Noto Serif and JetBrains Mono are bundled, so it looks the same on any system.

**Themes**
- Three looks, picked in Settings under Appearance. The switch is instant, no restart.
- Runout: the default dark verdigris and copper.
- Kawaii Light: cream and strawberry pink with mint and butter accents, rounded everything, a faint dot pattern behind the views, a pink pill for the open section, hearts for the seek and volume knobs, sparkles on the album header, and a kaomoji here and there.
- Kawaii Dark: the same thing on cocoa plum, for night.
- The kawaii themes switch the type to M PLUS Rounded 1c and Zen Maru Gothic (both bundled) and put the Japanese name next to each sidebar section (アルバム, ラジオ, プレイリスト).
- The TUI follows the setting too, with the same colours in the terminal.

**Bit-perfect**
- Off by default. Turn it on in Settings, under Bit-perfect, and local files go straight to the DAC: an ALSA hardware device, opened exclusively, at the file's own sample rate and bit depth, with nothing in between. No resampling, no software volume, no dither, and no silence slipped in.
- Turning it on asks first, since the card is PatinaPlayer's alone while it plays: other apps can't use it until you pause, stop, or turn this off.
- On a PipeWire desktop it asks PipeWire to let go of the card first (the standard D-Bus device reservation, the same thing JACK does), and PipeWire takes the card back as soon as bit-perfect closes it.
- If the DAC can't take the file as it is (a 24/192 file on a 16/48 DAC, say), it refuses and says why ("the DAC doesn't support 24/192"), then plays through the normal output so the music doesn't stop. It never resamples quietly.
- The bottom bar shows BIT-PERFECT while it's happening. NOT BIT-PERFECT in copper means it was refused; click it for the reason. EXCLUSIVE means a lossy source (MP3, AAC) going out untouched as integers, which is as good as it gets for those.
- Volume goes to the DAC's own volume control if it has one. If it doesn't, volume is fixed at 100% and the slider greys out. Mute still mutes.
- Radio and podcasts never play bit-perfect. Radio isn't a file, and a sped-up podcast is processed by definition.
- Auto mode means "bit-perfect whenever nothing would change the sound". There's no EQ or leveling yet, so for now it's the same as On.
- Pick the card under Bit-perfect device. The list comes from ALSA when you open Settings, so a USB DAC you just plugged in shows up.

**Settings**
- Every option in config.toml can be changed from the Settings view in either app, and is written straight back to the file with your comments kept.
- Library folders: add one with the desktop's own folder picker (Browse) or by typing a path, and the scan starts right away. Remove one and its tracks leave the index. The files stay where they are.
- Output, device and buffer changes reopen the output on the spot, and a playing song carries on from where it was. So does switching bit-perfect on or off.
- In the TUI: Enter flips, cycles or edits; `h` and `l` step a value down or up; `N` adds a folder; `d` removes one.

**Play, shuffle, queue or keep wherever you are**
- Every library view has Play, Shuffle, Add to queue and Add to playlist at the top (`v`, `V`, `e` and `P` from the keyboard, in both apps). They take the whole view: a folder means everything under it, subfolders and all, an artist means all their albums, Albums or Songs means the library.
- Add to playlist offers New playlist… first, then every playlist you have, so a view can start a playlist or go on the end of one.
- On a narrow window Add to queue and Add to playlist shrink to their icons, so the row still fits.
- Go into a folder of a few hundred albums, hit Shuffle, and it plays through all of them in random order. Hit Play and it plays them in folder order.
- Shuffle leaves the shuffle button on and Play turns it off, so the button flips between the two afterwards without losing your place.
- In a grouped list (search results, Most listened) only the songs play, since the albums and artists there would repeat them.
- Radio, Podcasts, Genres and Playlists don't get the buttons, since they're lists of lists. Open one and it does.

**Playlists and the queue**
- The queue goes into a playlist from the Add to playlist button under the queue (or `S` in the Queue view): a new one, or any playlist you already have.
- Only songs from your library go in. Radio stations and podcast episodes in the queue are left out, since a playlist can't hold them.
- Adding never doubles a song up. Anything already in the playlist, or in the queue twice, goes in once, and the note says how many were left out.

**Listening stats**
- Three views under Listening in the sidebar, built from your play counts.
- Most listened: your top five songs, top five albums and top five artists.
- Top albums: the ten albums whose songs you've played most, added up.
- Top artists: the ten artists whose songs you've played most, added up. They group the same way the Artists view does, so `t` in Artists switches these too.
- They update as you listen, and open like any other album or artist.

**Cover art**
- Embedded art first, then a cover image in the folder (`cover`, `folder`, `front`, `album` and the like, as jpg, png or webp). Disc folders look in their parent too.
- The desktop app loads it on background threads, and only for the rows on screen, so a 3000-album grid or a 5000-track queue doesn't load every cover at once.
- The TUI draws it at the top of the queue panel, with kitty graphics, sixel or iTerm2 images where the terminal supports them and half blocks everywhere else. `--art` picks one by hand.

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
- Playlists: create, rename, delete, add to from anywhere (a song, an album, a whole view, the queue), reorder and remove items.
- Queue: what's playing and next, with reorder, remove, clear and add to a playlist.
- Radio: your stations, with add, edit, test, reorder and delete.
- Podcasts: your subscriptions, then each show's episodes with what's new, how far you got, and what's downloaded.
- Downloads: downloaded podcast episodes. The track cache joins it in milestone 7.
- Settings: every option, editable. See above.
- Search: type-ahead over the index, artists, albums and songs at once. Every word has to match somewhere, so `nirv blea` finds Bleach.
- A queue panel (`q`) that shows what's coming up beside any view.

**Internet radio**
- Any direct stream FFmpeg can decode: MP3, AAC and HE-AAC, Ogg Vorbis, Opus, FLAC over HTTP. Icecast and SHOUTcast included.
- Playlist links resolve on their own. Paste a `.pls`, `.m3u` or `.m3u8` URL and the player reads it, pulls out the stream URLs, and plays the first one that answers. A playlist that points at another playlist is followed too.
- HLS streams (an `.m3u8` with segments) go straight to FFmpeg.
- Test a station with `T` and the list shows what the URL turned out to be: `Playlist, 3 mirrors, AAC 128 kbps, 48 kHz stereo`, or why it isn't working. The test plays a second of the stream, so "working" means it actually decodes, not just that the server answered. New stations get tested as soon as you save them.
- Stations that switch audio format mid-stream keep playing. HE-AAC streams (a lot of public broadcasters use them) announce half their real sample rate and often mono, then decode to the full rate in stereo; ad breaks sometimes switch rates too. The player now follows the switch instead of treating it as a dropped connection and reconnecting forever.
- Now-playing titles from ICY metadata show in the bottom bar. HLS timed ID3 titles are read too where FFmpeg exposes them, though that path hasn't been tried against a real station yet.
- A dropped connection reconnects on its own with backoff (1, 2, 4, 8, 16, then every 30 seconds), going through the mirrors again each time. The bottom bar says when the next try is.

**Podcasts**
- Subscribe with `N` and a feed URL. Episodes are fetched in the background on startup and on `F5`.
- Picks up where you left off. Your position is saved every few seconds and on pause, and resuming backs up five seconds so you catch the sentence again. Finished episodes are marked played.
- Speed from 0.5x to 3x with pitch kept (`[` and `]`), and silence trimming (`z`), which cuts any pause over 0.7 seconds down to a quarter second. Both apply to podcasts only. Music and radio never change speed.
- Download episodes for offline with `w`. An interrupted download picks up where it stopped, and a half-finished file never takes the real name, so a broken download can't pass for a finished one.
- Streams episodes you haven't downloaded, so you don't have to wait.

**Playback**
- Anything FFmpeg decodes: FLAC, ALAC, WAV, AIFF, APE, WavPack, MP3, AAC, Opus, Vorbis and the rest.
- Gapless playback. A test proves FLAC joins are sample-exact, and another proves MP3 padding is trimmed.
- Short fades on pause, resume, seek, next and stop. PipeWire output with ALSA fallback.
- Play counts. A track counts once you've actually listened to half of it or four minutes, whichever comes first. Seeking past the middle doesn't count.

## The AppImage

GitHub builds it. Push a tag and you get a release with the AppImage on it:

1. Push the repo to GitHub, with `.github/workflows/appimage.yml` in it.
2. Tag a version and push the tag: `git tag v0.2.0 && git push origin v0.2.0`
3. Wait for the AppImage workflow under the Actions tab. It takes about 15 minutes the first time and less after, since FFmpeg and the Rust build are cached.
4. The release shows up under Releases with `PatinaPlayer-0.2.0-x86_64.AppImage` and its `.zsync` file.

For a test build without a release, run the workflow by hand from the Actions tab (Run workflow). The AppImage lands as a downloadable artifact on that run.

Using it:

1. Make it executable: `chmod +x PatinaPlayer-*-x86_64.AppImage`
2. Run it: `./PatinaPlayer-0.2.0-x86_64.AppImage` for the desktop app.
3. For the terminal app: `./PatinaPlayer-0.2.0-x86_64.AppImage --tui`, or symlink it as `patinaplayer-tui` and run that, like `ln -s ~/Applications/PatinaPlayer-0.2.0-x86_64.AppImage ~/.local/bin/patinaplayer-tui`.
4. For a menu entry, use Gear Lever or AppImageLauncher (or whichever AppImage manager you like). The desktop file and icon are inside. They can also update it from the `.zsync`, since the update info points at your repo's latest release.

What's in it, and what isn't:

- FFmpeg is built from source, audio only, and linked into the binaries. Every audio decoder, every demuxer, HTTPS through GnuTLS. No video codecs, no encoders. That keeps the AppImage small and independent of whatever FFmpeg your distro has.
- The FFmpeg version follows the Rust bindings in `Cargo.lock`, so a `cargo update` that moves them to a newer FFmpeg moves the AppImage to it too. Nothing is pinned. linuxdeploy is the newest build on every run.
- PipeWire's client library, GnuTLS, ALSA, OpenGL, fontconfig, freetype and glibc come from your system, since bundled copies of those break against the host. Every desktop distro new enough to run it has them.
- IT NEEDS GLIBC 2.39 OR NEWER, since it's built on Ubuntu 24.04: Kubuntu 24.04, Fedora 40, Debian 13 and later are fine. Older systems get a glibc error and nothing else.

To build one locally instead: `bash packaging/build-appimage.sh`. You need everything from Prerequisites plus `nasm`, `libgnutls28-dev` and `zlib1g-dev` (or whichever names your distro uses). The result is in `target/appimage/`.

A note on how albums are grouped. Tracks with an album artist tag group by album artist plus album name, wherever the files sit. Tracks without one group by album name plus folder, which keeps two unrelated "Greatest Hits" apart and keeps a compilation together even when every track has a different artist. Disc folders (`CD1`, `Disc 2`) are folded into their parent, so multi-disc albums come out as one.

## Prerequisites

- Rust, latest stable. I use rustup, so in my case that's `rustup update stable`.
- FFmpeg 5.1 or newer, with dev headers
- PipeWire and ALSA dev headers
- clang and pkg-config, since the FFmpeg and PipeWire bindings are generated at build time
- For the AppImage only: nasm, GnuTLS and zlib headers, since it builds its own FFmpeg
- For the desktop app: xkbcommon, Wayland, fontconfig, freetype, the X11 cursor, randr and input libraries, and OpenGL headers

SQLite is built into the binary, so there's nothing to install for it.

On Kubuntu (or any Ubuntu 24.04 based system):

```
sudo apt install pkg-config clang libavcodec-dev libavformat-dev libavutil-dev libswresample-dev libavfilter-dev libavdevice-dev libpipewire-0.3-dev libspa-0.2-dev libasound2-dev \
  libxkbcommon-dev libwayland-dev libfontconfig1-dev libfreetype-dev libx11-dev libxcursor-dev libxrandr-dev libxi-dev libgl1-mesa-dev
```

If you only want the TUI, skip the second line and build with `-p patina-tui`.

On another distro the package names differ but the list is the same: FFmpeg, PipeWire, libspa, ALSA, clang. Fedora calls them `ffmpeg-free-devel` (or `ffmpeg-devel` from RPM Fusion), `pipewire-devel` and `alsa-lib-devel`, for example. You'll have to check yours.

The tests also need the `ffmpeg` command itself, since they generate their own audio files.

## Building

1. Extract the archive and go into it: `cd patinaplayer`
2. Build both: `cargo build --release`. The first build takes a while, since Slint compiles a lot.
3. Copy the binaries somewhere on your PATH: `cp target/release/patinaplayer target/release/patinaplayer-tui ~/.local/bin/`, since that's where user binaries usually live.
4. Optionally run the tests: `cargo test --workspace`

Built this way there's no menu entry. Start it from a terminal or KRunner (or whichever launcher you use) by name, or copy `assets/patinaplayer.desktop` to `~/.local/share/applications/` and `assets/patinaplayer.png` to `~/.local/share/icons/hicolor/256x256/apps/`. The AppImage below carries both.

## First run

Everything below works the same with `patinaplayer` or `patinaplayer-tui`. They share the config and the library, though you'd only run one at a time.

1. Run it once: `patinaplayer`. That writes `~/.config/patinaplayer/config.toml`. If your system has a Music folder (`xdg-user-dir MUSIC`), it's already in the list.
2. Add any other folders in Settings, under Library: Browse for one, or type its path. The scan starts right away, with progress next to the folder in the sidebar, and the views fill in as it goes.

The command line does the same, if you'd rather: `patinaplayer --add-folder /mnt/storage/music` adds one and exits, and `--remove-folder` takes one out.

You can still play files directly without touching the library: `patinaplayer song.flac ~/Downloads/SomeAlbum`. They go straight into the queue.

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
bit_perfect = "off"      # off, on or auto
bit_perfect_device = ""  # like "hw:CARD=DAC,DEV=0"; empty is the first card

[appearance]
theme = "runout"         # runout, kawaii-light or kawaii-dark

[playback]
volume = 80              # starting volume, 0 to 100
fade_ms = 100            # 0 turns fades off

[podcasts]
feeds = ["https://example.com/feed.xml"]
speed = 1.0              # 0.5 to 3, podcasts only
trim_silence = false

[[radio.station]]
name = "Some Station"
url = "https://example.com/listen.pls"
```

Settings, Radio and Podcasts edit this file for you, and keep the rest of it as you wrote it.

A typo in a key name is an error, not a silent default, so you find out right away. Command line flags (`--backend`, `--device`, `--buffer-ms`, `--volume`, `--fade-ms`) override the file for one run.

Where everything else goes:

| What | Where |
|---|---|
| Settings | `~/.config/patinaplayer/config.toml` |
| Library index, playlists, play counts, podcast positions | `~/.local/share/patinaplayer/library.db` |
| Downloaded podcast episodes | `~/.local/share/patinaplayer/offline/podcasts/` |
| Resized cover art | `~/.cache/patinaplayer/art/` |

The index lives in `~/.local/share` on purpose, since cleaners like BleachBit wipe `~/.cache`, and nobody should lose their playlists to that. The art cache is safe to wipe. It refills from your files.

## Keys

These are the TUI keys. The desktop app uses the same ones, with these differences: there's no Tab focus switching, `Delete` removes from the queue, the open playlist, or the library folders in Settings, `Ctrl+F` also searches, `Alt+Left` and `Alt+Right` go back and forward, and the per-view letter commands (new station, rename, unsubscribe and so on) live in the right-click menu and the buttons at the top of each view instead.

| Key | Action |
|---|---|
| Space | Play / pause |
| n / b | Next / back (back restarts the track if you're more than 3 seconds in) |
| Left / Right | Seek 5 s, Shift for 30 s |
| + / -, m | Volume, mute |
| s, r | Shuffle, cycle repeat |
| 1 to 9, 0 | Folders, Artists, Albums, Songs, Genres, Playlists, Radio, Podcasts, Queue, Downloads |
| Tab | Switch between the sidebar and the list |
| / | Search |
| [ / ] | Podcast speed down / up |
| z | Podcast silence trimming |
| Enter | Open, or play from the selected track |
| v / V | Play the whole view in order / shuffled |
| e | Add the whole view to the queue |
| P | Add the whole view to a playlist |
| Esc / Backspace | Back up a level |
| a / A | Add to the queue / play next |
| p | Add to a playlist |
| o / O | Sort Albums or Songs / reverse Songs |
| t | Artists: album or track artist. Genres: albums or songs |
| q | Queue panel |
| . | Jump to the playing track in Queue |
| F5 | Check folders for changes and refresh podcast feeds |
| ? | Help |
| Ctrl+Q | Quit |

In Radio: `N` new station, `E` edit, `T` test, `D` delete, `J`/`K` move. In Podcasts: `N` subscribe, `D` unsubscribe. In a show's episodes: Enter plays (resuming), `w` downloads (or cancels), `x` deletes the downloaded file, `P` toggles played.

In the Queue: `d` remove, `J`/`K` move down or up, `S` add the queue to a playlist (new or existing), `C` clear. In Settings: Enter flips, cycles or edits the option, `h`/`l` step it down or up, `N` adds a library folder, `d` removes one. In Playlists: `N` new, `R` rename, `D` delete, and inside a playlist `d` and `J`/`K`.

Pressing Enter on a track plays the list it's in from that track, so Enter on track 3 of an album plays the album from track 3. Use `a` if you'd rather add to what's already queued.

The mouse works in the TUI too: click to select, double-click to play or open, right-click to go back, and click or drag the seek and volume bars.

TUI cover art picks a protocol on its own by asking the terminal. To force one: `patinaplayer-tui --art kitty` (or `sixel`, `iterm2`, `blocks`, or `off`). Konsole supports kitty images, so on Kubuntu auto should land there. Inside tmux, auto usually falls back to blocks.

## How it's put together

```
crates/
  patina-core/          shared core, used by both frontends
    src/audio/          decoder, gapless feeder, real-time renderer, outputs
    src/library/        SQLite index, tag reading, scanner, inotify service, podcast rows
    src/browse/         every view and every action on it, shared by both frontends
    src/session.rs      startup both frontends share: config, library, player
    src/art.rs          finding, cropping and caching cover art
    src/radio.rs        station URL resolving: pls, m3u, HLS detection, mirror checks
    src/podcast.rs      feed fetching, episode downloads with resume
    src/config.rs       config file and XDG paths
    src/player.rs       core thread: commands in, snapshots and events out
    src/queue.rs        queue on stable entry ids, shuffle, repeat
    examples/scan.rs    time an index of your own folders
    tests/              gapless, seek, library, watcher, player, filter, network and browser tests
  patina-tui/           the terminal frontend (ratatui, ratatui-image)
  patina-gui/           the desktop app (Slint)
    ui/app.slint        the whole window
    ui/fonts/           bundled fonts and their license
    src/bridge.rs       turns the shared browser into Slint models and back
assets/                 app icon and desktop file
packaging/              AppImage: FFmpeg build, AppDir, entry point
.github/workflows/      CI on every push, AppImage on every tag
```

The core owns all the state. The views and what each action does live in `patina-core/src/browse`, so the TUI and the desktop app are only drawing and input. A fix to how a view behaves lands in both at once.

The library runs on its own thread with its own database connection. SQLite is in WAL mode, so the views keep reading while a scan writes. Paths are stored as raw bytes, since Linux file names don't have to be valid UTF-8.

The queue refers to entries by id, never by position. That's what lets you reorder or remove tracks while the decoder is already reading the next one, without anything getting confused.

To see how fast indexing is on your own collection: `cargo run --release -p patina-core --example scan -- ~/Music`. It uses a throwaway database, so your real index isn't touched.

The bit-perfect test plays a 16/44.1 and a 24/96 FLAC of white noise to ALSA's `file` plugin, which writes everything it's sent to a file, then compares that file byte for byte with FFmpeg's own decode. Any resampling, volume, dither or padding anywhere in the chain fails it. No sound card needed.

The network tests run a small HTTP server inside the test itself: an ICY stream that hangs up on purpose to check reconnects, a `.pls` with a dead first mirror, real HLS segments, an RSS feed, and a download that breaks halfway to check resume. No internet needed.

## Known gaps in this milestone

- Podcasts and stations have no art yet. Podcast feed images and station logos come later, cover art for tracks works now.
- Bit-perfect was checked bit for bit here through ALSA's file plugin, at 16/44.1 and 24/96, and the PipeWire hand-off was checked against a stand-in on a private D-Bus. It hasn't touched a real DAC or a real PipeWire from this box. CI also tries a real ALSA kernel driver (snd-aloop) when the runner can load it.
- In bit-perfect mode, going between tracks of different formats (16/44.1 into 24/96) closes and reopens the card, so there's a short gap. Same-format tracks are gapless as always.
- The Browse folder picker goes through the desktop portal (xdg-desktop-portal), which Plasma, GNOME and most desktops run. Without it, Browse says so and Type a path still works.
- Command line flags like `--backend` still override the file for one run, but Settings shows the file's values, not the override.
- No tray icon and no MPRIS media keys yet. Those are milestone 14.
- The kawaii fonts are bundled as subsets (Latin, kana and symbols), which saves about 3 MB per weight. Kanji in your own tags still show through your system's Japanese font, so a Japanese title can mix two typefaces. With no Japanese font installed at all, kanji show as boxes in every theme, the same as before.
- The themes were checked under X11 (Xvfb) here, at full width and narrow, like the rest of the desktop app.
- The AppImage build was run end to end on Ubuntu 24.04 here, the same system as the GitHub runner, but the workflow itself hasn't run on GitHub yet. The first tagged run is the real test.
- The desktop app was checked under X11 (Xvfb) here, not on a real Plasma Wayland session. Slint supports Wayland natively, so it should be fine, but that's untested from this box.
- When the next track has a different sample rate (44.1 kHz into 48 kHz, say), the output reopens at the new rate and there's a short gap. Gapless across rates needs the resampler, which is milestone 12.
- Built from source the normal way, both binaries link your system's FFmpeg, PipeWire and ALSA. The AppImage links its own FFmpeg in.
- 16-bit ALSA output is rounded, not dithered. TPDF dither lands with the output controls in milestone 12, and it only matters on the ALSA fallback with a device that refuses float and 32-bit.
- Radio has no loudness leveling yet. The live normalizer for radio comes with leveling in milestone 9.
- Stations and podcasts from a Subsonic server come with the Subsonic client in milestone 6. For now it's only the ones you add.
- Scrobbling radio titles comes with Last.fm in milestone 11, off by default since station metadata is often junk.
- Podcast position at a sped-up or trimmed speed is tracked from markers the decoder sends a few times a second, so the saved spot can be off by a fraction of a second. Resume backs up five seconds anyway.
- The radio and podcast code was tested against local servers, including a real HLS stream made by FFmpeg. It hasn't been run against live stations from here, since this build box has no general internet access. HTTPS streams go through FFmpeg's own TLS, so a source build needs an FFmpeg built with it (distro packages are). The AppImage's FFmpeg has GnuTLS.
- inotify needs one watch per folder. A very large library can run past `fs.inotify.max_user_watches`. If it does, you'll see an error saying so, and raising the limit with `sysctl` fixes it.

## License

GPL-3.0-or-later. See `LICENSE`.

The bundled fonts (Inter, Noto Serif, JetBrains Mono, M PLUS Rounded 1c, Zen Maru Gothic) are under the SIL Open Font License, in `crates/patina-gui/ui/fonts/`.

Now PatinaPlayer turns any view or the queue into a playlist, queues whole views in one click, and wears pink when you ask it to.
