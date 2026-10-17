# PatinaPlayer

PatinaPlayer is a music player for Linux and Android that plays your local folders and your Subsonic, Ampache and DLNA servers through one library, for people who want a quiet interface on top of a serious playback chain.

It comes as two apps on one core: a desktop app (Slint) that's also the Android app, and a terminal app (ratatui) that does all the same things from a keyboard. They share the config, the library and every view, so a fix to how something behaves lands in both.

## What it does

### Your music, wherever it lives

- **Local folders**, indexed into one SQLite database, so browsing is instant and works offline. Tags come from lofty (ID3v2, Vorbis comments, MP4, APE), with FFmpeg as the fallback. A rescan only reads new or changed files, and the folders are watched while the app runs, so an album you copy in shows up a couple of seconds later.
- **Subsonic servers** (Navidrome, Gonic, Airsonic, LMS and anything else that speaks the API) and **Ampache**, as many as you like. The app logs in and works out which kind it is, so there's nothing to pick. The server's library is copied into the local index, so its songs sit in every view next to your local music and browsing never waits on the network.
- **DLNA servers** (MiniDLNA, Gerbera, Jellyfin, Plex, Serviio, Twonky, most NAS boxes). Find DLNA servers in Settings asks the network who's there. No address to type, no login.
- Server playlists, play queues (Subsonic), radio stations and podcasts come across too. Changes to a server playlist go back to the server.
- Ratings and starred albums and artists sync both ways with Subsonic and Ampache.
- Passwords live in the desktop keyring (KWallet, GNOME Keyring) through the Secret Service API, never in the config file. With no keyring (a TTY, SSH, a phone) they go in a file only you can read, and the app says so.

### Views

- Folders, Artists (by album artist or track artist), Albums (by name, artist, year, recently added, most played, rating or random), Songs (sortable on seven columns), Genres, Playlists, Radio, Podcasts, Queue and Downloads.
- Search over everything at once. Every word has to match somewhere, so `nirv blea` finds Bleach.
- Listening stats: Most listened (top five songs, albums and artists), Top albums and Top artists, built from your play counts. A song counts once you've heard half of it or four minutes, whichever comes first.
- Favorites: star a folder, album or artist and it sits in the sidebar, one click away.
- Right-click a song (long press on a phone), in any list or in the queue, for Go to album and Go to artist.
- Every library view has Play, Shuffle, Add to queue and Add to playlist at the top, and they take the whole view. Shuffle on a folder of three hundred albums plays all of them in random order.
- Ratings, 1 to 5 stars, on songs and albums.
- Lyrics, only when you press the Lyrics button: the song's tags, an `.lrc` next to it, its server, then LRCLIB if you've turned that on. Synced lyrics follow the song.

### Playback

- Anything FFmpeg decodes: FLAC, ALAC, WAV, AIFF, APE, WavPack, MP3, AAC, Opus, Vorbis and the rest.
- Gapless, sample-exact. The tests prove FLAC joins match the source and MP3 padding gets trimmed.
- Crossfade from 0 to 12 seconds, never between songs that follow on from each other on an album, so live albums and DJ mixes stay gapless.
- Sample rate set to Match (each album at its own rate) or a fixed rate, with three resampler qualities. Songs on one album that change rate are resampled to keep the join gapless.
- PipeWire output with ALSA fallback on the desktop, AAudio on Android. Pick the output from the bottom bar and the song carries on from where it was.
- Clearing the queue takes away what's coming up. The song playing finishes, then nothing plays.

### The sound chain

In order, after decoding: leveling, the amp stack, the EQ, the channel tools, volume, then the limiter.

- **Volume leveling**, the ReplayGain 2 way, to -18 LUFS by default. Smart mode uses the album gain while an album plays in order and the track gain on shuffle and in mixes. Gains come from the server, then the file's tags, then the app's own EBU R128 measurement, made once per song and kept. Radio and podcasts get a live normalizer instead.
- **Amp stack**: a rack of old hardware you chain in any order. Tube preamp, tube power amp, output transformer, tape deck, phono stage, console channel, bus compressor, exciter and bass enhancer, each with its own knobs, oversampled 4x by default. Off by default and subtle at its defaults. They're character effects built from how each kind of hardware behaves, not models of branded units.
- **EQ**: graphic (10, 15 or 31 bands) or parametric (up to 20 bands of peak, shelf, pass and notch filters). Imports AutoEq and Equalizer APO files. Presets, built-in and your own. Each output device can keep its own EQ, and on PipeWire it switches with the default output, mid-song.
- **Channel tools**: mono, stereo width, crossfeed for headphones (bs2b style) and balance.
- **True-peak limiter**, last in the chain, at -1 dBTP by default, so EQ boosts and leveling never clip. Below the ceiling it doesn't touch a sample.
- **Bit-perfect**, off by default. Local files and original server files go straight to the DAC, opened exclusively, at the file's own rate and depth, with nothing in between. On a PipeWire desktop it asks PipeWire to let go of the card first. If the DAC can't take the file as it is, it says why and plays through the normal output instead. It never resamples quietly. Auto mode turns it on only when nothing else would change the sound.

Everything in the chain glides when you change it, so nothing clicks or zippers.

### Servers on a slow line

- Every server song plays through a cache. Prefetch downloads what's coming up in the queue (by size, 1 GB by default, or by song count), the cache keeps what you've played (10 GB by default), and Keep offline pins songs, albums, artists, playlists or server folders until you unpin them.
- A song that isn't on disk yet plays from its download as it comes in. If playback catches up with the download, it waits for a few seconds of the song to build up, or for enough that the rest plays through without another stop, instead of stuttering through every scrap that arrives.
- Downloads resume where they stopped. Background downloads hold on a metered connection (through NetworkManager).
- A server that stops answering goes grey instead of vanishing. Pinned and cached songs keep playing.
- Songs come as the original file by default. A server can be switched to Opus or MP3 at 96 to 320 kbps, for streaming or for what's kept.

### Mixes

- Genre mix: mark one or more genres and play every song on the albums tagged with all of them, shuffled.
- Artist mix and track mix: the artist or song plus similar ones, endless, topped up as the queue runs low.
- Who's similar comes from your server first, then Last.fm (with an API key), then your own genre tags, which works with local folders and no key at all. Only songs you have get played.
- Save a mix you like. It remembers who was similar, so it plays the same way later without the server or Last.fm.

### Last.fm

- Link your account from Settings. Now Playing, scrobbles by Last.fm's rules, and love (the heart) that's kept in the library and sent to Last.fm.
- Scrobbles made offline wait in the library and go out later with their original times.
- For each server you choose who scrobbles: the app, the server, or both. Navidrome and others pass plays on themselves, so picking the right one keeps plays from counting twice.

### Radio and podcasts

- Internet radio: any stream FFmpeg can decode, Icecast and SHOUTcast included. `.pls`, `.m3u` and `.m3u8` links resolve on their own, HLS goes straight to FFmpeg, and a dropped stream reconnects with backoff. Test a station and it tells you what the URL turned out to be.
- Podcasts: subscribe by feed URL, pick up where you left off, 0.5x to 3x speed with the pitch kept, silence trimming, and downloads for offline.

### The desktop

- MPRIS2, so media keys, Plasma's and GNOME's media controls and `playerctl` all work, with the cover.
- A tray icon (StatusNotifierItem). Closing the window hides it there and the music keeps going, unless you'd rather close meant quit.
- The window steps down from a wide desktop to phone size without cutting anything off. A row of buttons that doesn't fit scrolls sideways. The sidebar shows names when there's room and folds to icons when there isn't. The button at the bottom of the sidebar, or Settings, Appearance, sets it to stay expanded or folded, separately for landscape and portrait.
- Themes: Runout (dark verdigris and copper, the default), Light, Dark, OLED (true black), and Kawaii Light and Kawaii Dark (pastel, rounded, with Japanese section names). The terminal app follows the theme too.
- Fonts are bundled, so it looks the same everywhere.

### The terminal app

- Every view, the EQ, the amp stack, Settings and the rest, from the keyboard, with mouse support.
- Cover art with kitty graphics, sixel or iTerm2 images where the terminal supports them, and half blocks everywhere else.
- Keeps playing when you detach tmux or screen.

### On Android

- The same window and the same core, for arm64 phones on Android 9 or later.
- Your Music folder is in the library from the first start. Add more with Android's folder picker.
- Tap opens and plays, long press opens the menu that right-click opens on the desktop. Text is typed in Android's own dialogs, which stay clear of the keyboard.
- Music keeps playing with the screen off, with the notification, lock screen controls, Bluetooth buttons, and pausing when headphones come out or a call comes in.
- Android Auto shows what's playing, with next, previous and play/pause. Music is picked on the phone, since the car has nothing to browse yet.
- The System theme (Android 12 and later) follows your wallpaper's Material You colors and the phone's light or dark mode.
- The volume button opens a tall slider, easier to hit with a thumb than the bar's.
- Bit-perfect to a USB DAC on Android 14 and later, where the phone's maker offers it.

## Getting it

### The AppImage

Download `PatinaPlayer-<version>-x86_64.AppImage` from Releases.

1. Make it executable: `chmod +x PatinaPlayer-*-x86_64.AppImage`
2. Run it: `./PatinaPlayer-*-x86_64.AppImage`
3. For the terminal app, run it with `--tui`, or symlink it as `patinaplayer-tui` somewhere on your PATH, like `ln -s ~/Applications/PatinaPlayer-0.1.0-x86_64.AppImage ~/.local/bin/patinaplayer-tui`.
4. For a menu entry, use Gear Lever or AppImageLauncher, or whichever AppImage manager you like. The desktop file and icon are inside, and they can update it from the `.zsync` file on the release.

IT NEEDS GLIBC 2.39 OR NEWER, since it's built on Ubuntu 24.04. Kubuntu 24.04, Fedora 40, Debian 13 and later are fine. Older systems get a glibc error and nothing else.

FFmpeg is built into it, audio only, so your distro's FFmpeg doesn't matter. PipeWire, ALSA, GnuTLS, OpenGL, fontconfig and glibc come from your system.

### The APK

Download `PatinaPlayer-<version>-arm64.apk` from Releases on your phone.

1. Open it. Android asks to let your browser (or file manager) install apps; allow it for that one app.
2. Open PatinaPlayer and allow music and notifications when asked.

It's arm64 only, which covers every phone from the last several years.

## Building from source

### Prerequisites

- Rust, latest stable. I use rustup, so in my case that's `rustup update stable`.
- FFmpeg 5.1 or newer, with dev headers
- PipeWire, libspa and ALSA dev headers
- clang and pkg-config, since the FFmpeg and PipeWire bindings are generated at build time
- For the desktop app: xkbcommon, Wayland, fontconfig, freetype, the X11 cursor, randr and input libraries, and OpenGL headers
- For the tests: the `ffmpeg` command itself, since they make their own audio files

SQLite is built into the binary, so there's nothing to install for it.

On Kubuntu, or anything based on Ubuntu 24.04:

```
sudo apt install pkg-config clang libavcodec-dev libavformat-dev libavutil-dev libswresample-dev libavfilter-dev libavdevice-dev libpipewire-0.3-dev libspa-0.2-dev libasound2-dev \
  libxkbcommon-dev libwayland-dev libfontconfig1-dev libfreetype-dev libx11-dev libxcursor-dev libxrandr-dev libxi-dev libgl1-mesa-dev
```

If you only want the terminal app, skip the second line and build with `-p patina-tui`.

On another distro the names differ but the list is the same. Fedora calls them `ffmpeg-free-devel` (or `ffmpeg-devel` from RPM Fusion), `pipewire-devel` and `alsa-lib-devel`, for example. You'll have to check yours.

### Steps

1. Clone it and go in: `git clone https://github.com/NullAngst/PatinaPlayer && cd PatinaPlayer`
2. Build both apps: `cargo build --release`. The first build takes a while, since Slint compiles a lot.
3. Copy them onto your PATH: `cp target/release/patinaplayer target/release/patinaplayer-tui ~/.local/bin/`, since that's where user binaries usually live.
4. For a menu entry, copy `assets/patinaplayer.desktop` to `~/.local/share/applications/` and `assets/patinaplayer.png` to `~/.local/share/icons/hicolor/256x256/apps/`.
5. Optionally, run the tests: `cargo test --workspace`

### The AppImage, locally

`bash packaging/build-appimage.sh`. It needs everything above plus `nasm`, `libgnutls28-dev` and `zlib1g-dev` (or whichever names your distro uses), since it builds its own FFmpeg. The result lands in `target/appimage/`.

### The APK, locally

`bash packaging/build-apk.sh`. It needs the Android SDK with build-tools and a platform of 34 or newer (`ANDROID_HOME`), the NDK (`ANDROID_NDK_ROOT`), JDK 17 or newer, cmake, and `rustup target add aarch64-linux-android`. It builds FFmpeg and Mbed TLS for Android first, then the Rust library, the Java side and the APK, without Gradle. The result lands in `target/apk/`.

Without a release key in the environment it signs with a debug key kept in `target/android-debug.keystore`, which is fine for your own phone. The script's header lists the four signing variables.

### Releases from GitHub

Push a tag and the Release workflow builds the AppImage and the APK and puts both on one release:

1. Tag a version: `git tag v0.2.0`
2. Push the tag: `git push origin v0.2.0`
3. Wait for the Release workflow under Actions. The first run takes about 15 minutes; later ones are faster, since FFmpeg and the Rust build are cached.

THE APK HAS TO BE SIGNED WITH THE SAME KEY EVERY RELEASE, or Android refuses to install it over the old one and the app's data is lost to a reinstall. Put the keystore and its passwords in the repository's secrets once. Until they're there, tag builds stop at the APK step on purpose, and the AppImage still ships.

A Last.fm API key and secret can be built in from the `LASTFM_KEY` and `LASTFM_SECRET` secrets. Without them, scrobbling needs your own key in Settings (free from last.fm/api/account/create).

To run the workflow without a release, start it by hand from the Actions tab. The builds land as artifacts on that run.

## First run

Both apps work the same here. They share the config and the library, though you'd only run one at a time.

1. Run it once: `patinaplayer` (or `patinaplayer-tui`). That writes `~/.config/patinaplayer/config.toml`, with your Music folder (`xdg-user-dir MUSIC`) already in it if your system has one.
2. Add other folders in Settings, under Library: Browse for one, or type its path. The scan starts right away, with progress next to the folder in the sidebar.

From the command line, if you'd rather: `patinaplayer --add-folder /mnt/storage/music` adds one and exits, and `--remove-folder` takes one out.

To add a Subsonic or Ampache server:

1. Open Settings and find Servers.
2. Choose Add server…
3. Give it a name for the sidebar, its address (whichever address you open its web page at, like `https://music.example.com`), your user name and your password. For an OpenSubsonic API key, leave the user name empty and put the key in the password box.
4. Connect. If the login works it's saved and the first sync starts, with progress next to its name in the sidebar.

To add a DLNA server:

1. Make sure it's on and on the same network as you.
2. Open Settings, find Servers, and choose Find servers… It listens for about three seconds.
3. Pick the server from the list. It's added under its own name and the sync starts.

If nothing answers, a firewall dropping incoming UDP is the usual cause, since the answers come back to a random port. Let replies from the server's address in through ufw, or whichever firewall you run.

To play files without the library: `patinaplayer song.flac ~/Downloads/SomeAlbum`. They go straight into the queue.

## Config

Everything is in `~/.config/patinaplayer/config.toml`, and every option can be changed from Settings in either app. Settings writes straight back to the file and keeps your comments. A typo in a key name is an error, not a silent default, so you find out right away.

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
rate = "match"           # or a rate in Hz, like 48000, to resample everything to
resampler = "balanced"   # fast, balanced or best
format = "auto"          # ALSA only: auto, f32, s32, s24 or s16
dither = true            # TPDF dither on 16 and 24-bit ALSA output
crossfade_ms = 0         # 0 to 12000

[appearance]
theme = "runout"         # runout, system, light, dark, oled, kawaii-light or kawaii-dark
sidebar_landscape = "auto"   # auto, expanded or folded
sidebar_portrait = "auto"

[playback]
volume = 80              # starting volume, 0 to 100
fade_ms = 100            # fades on pause, seek and skip; 0 turns them off

[leveling]
mode = "smart"           # off, track, album or smart
target_lufs = -18        # -23 to -10; higher is louder
fallback_db = -6         # -12 to 0, for songs not measured yet
clip_guard = true        # keep the gain under the song's peak
live_music = false       # the live normalizer for unmeasured music
live_radio = true        # the live normalizer for radio and podcasts

[limiter]
on = true
ceiling_db = -1.0        # -6 to 0 dBTP

[channel]
on = false               # mono, width, crossfeed and balance
mono = false
width = 100              # percent: 0 is mono, 200 the widest
crossfeed = "off"        # off, light, medium or strong
balance = 0              # -100 (left only) to 100 (right only)

[lyrics]
online = false           # ask LRCLIB when nothing local or on the server has them

[desktop]
tray = true
close_to_tray = true     # closing the window hides it there and keeps playing

[podcasts]
feeds = ["https://example.com/feed.xml"]
speed = 1.0              # 0.5 to 3, podcasts only
trim_silence = false

[cache]
size_mb = 10240          # cached and prefetched songs; 0 turns the cache off
prefetch = "size"        # size or tracks
prefetch_mb = 1024       # how far ahead, by size; can't be more than size_mb
prefetch_tracks = 20     # how far ahead, by songs
pause_on_metered = true  # hold background downloads on a metered connection
format = "raw"           # raw (the original), opus or mp3
bitrate = 192            # kbps, for opus and mp3

[mixes]
genre_match = "album"    # album (the album's tags) or track (each song's own)

[lastfm]
api_key = ""             # your own key; empty uses the built-in one, if the build has it
api_secret = ""
user = ""                # the account linked; Settings fills it in
scrobble = true
scrobble_radio = false   # from stream titles, once one's been on a minute
love_stars = false       # love also stars the song on its server

[lastfm.servers]         # who scrobbles each server's songs: app, server or both
Home = "server"

[[radio.station]]
name = "Some Station"
url = "https://example.com/listen.pls"

[[server]]
name = "Home"            # shown in the sidebar
url = "https://music.example.com"
user = "me"
kind = "subsonic"        # subsonic, ampache or dlna; Settings finds out
stream = "raw"           # raw (the original file), opus or mp3
bitrate = 192            # kbps, for opus and mp3
```

The app writes the `[eq]` and `[amp]` tables itself, along with which device gets which preset. The EQ and Amp views are the easier way in. Command line flags (`--backend`, `--device`, `--buffer-ms`, `--volume`, `--fade-ms`) override the file for one run.

Where everything else goes:

| What | Where |
|---|---|
| Settings | `~/.config/patinaplayer/config.toml` |
| EQ and amp stack presets | `~/.config/patinaplayer/presets/eq/` and `presets/amp/` |
| Library index, playlists, play counts, ratings, podcast positions, loudness, waiting scrobbles | `~/.local/share/patinaplayer/library.db` |
| Songs kept offline, downloaded episodes | `~/.local/share/patinaplayer/offline/` |
| Cached and prefetched songs | `~/.cache/patinaplayer/tracks/` |
| Cover art at the sizes the app draws | `~/.cache/patinaplayer/art/` |
| Server passwords, the Last.fm session | your keyring, or `~/.config/patinaplayer/secrets.toml` (only you can read it) when there's no keyring |

The index and the offline songs live in `~/.local/share` on purpose, since cleaners like BleachBit wipe `~/.cache`, and nobody should lose their playlists or a pinned library to that. The two caches are safe to wipe. They refill as you play.

On Android all of it lives in the app's private storage.

## Keys

These are the terminal app's keys. The desktop app uses the same ones, except there's no Tab focus switching, `Delete` removes from the queue or the open playlist, `Ctrl+F` also searches, `Alt+Left` and `Alt+Right` go back and forward, and the per-view letter commands live in the right-click menu and the buttons at the top of each view. `?` lists them in either app.

| Key | Action |
|---|---|
| Space | Play / pause |
| n / b | Next / back (back restarts the song if you're more than 3 seconds in) |
| Left / Right | Seek 5 s, Shift for 30 s |
| + / -, m | Volume, mute |
| s, r | Shuffle, cycle repeat |
| 1 to 9, 0 | Folders, Artists, Albums, Songs, Genres, Playlists, Radio, Podcasts, Queue, Downloads |
| Tab | Switch between the sidebar and the list |
| / | Search |
| Enter | Open, or play the list from the selected song |
| Esc / Backspace | Back up a level |
| v / V | Play the whole view in order / shuffled |
| e | Add the whole view to the queue |
| P | Add the whole view to a playlist |
| a / A | Add to the queue / play next |
| i / I | Go to the selected song's album / artist (in the Queue too) |
| p | Add to a playlist |
| o / O | Sort Albums or Songs / reverse Songs |
| t | Artists: album or track artist. Genres: albums or songs |
| f / F | Keep the selection / the whole view offline, or stop |
| X | Mix from the selection: an artist, album, song, or the marked genres |
| M | Genres: mark a genre for a mix. Queue: save the mix playing |
| q | Queue panel |
| l | Love the song playing |
| `*` | Star the selected folder, album or artist |
| y / Y | Rate the selected song or album / the song playing |
| [ / ], z | Podcast speed down / up, silence trimming |
| . | Jump to the playing song in Queue |
| Ctrl+E, Ctrl+A | The EQ, the amp stack |
| Ctrl+O | Pick the output device |
| Ctrl+L | Lyrics for the song playing |
| F5 | Check folders for changes and refresh podcast feeds |
| Ctrl+Q | Quit |

In Radio: `N` new station, `E` edit, `T` test, `D` delete, `J`/`K` move. In Podcasts: `N` subscribe, `D` unsubscribe; in a show, `w` downloads, `x` deletes the download, `P` marks played. In the Queue: `d` remove, `J`/`K` move, `S` save to a playlist, `L` load the queue saved on your server, `C` clear. In Playlists: `N` new, `R` rename, `D` delete. In Downloads: `w` pauses or resumes, `x` cancels or unpins, `W` pauses everything, `C` clears the cache. In Settings and the EQ: Enter flips, cycles or edits, `h`/`l` step a value down or up.

TUI cover art picks a protocol by asking the terminal. To force one: `patinaplayer-tui --art kitty` (or `sixel`, `iterm2`, `blocks`, `off`). Konsole supports kitty images. Inside tmux, auto usually falls back to blocks.

## How it's put together

```
crates/
  patina-core/      everything that isn't drawing: audio, library, servers, cache, DSP, views
  patina-gui/       the desktop app (Slint), and the window the Android app shows
  patina-tui/       the terminal app (ratatui)
  patina-android/   the Android app's native library and its JNI glue
android/            manifest, icons and the Java side (permissions, media session, Android Auto, devices)
assets/             app icon and desktop file
packaging/          AppImage and APK builds, FFmpeg for each
.github/workflows/  tests on every push, real servers in containers, releases on tags
```

The core owns all the state, and every view and what each action does lives in `patina-core/src/browse`, so both frontends only draw and take input. The library runs on its own thread with SQLite in WAL mode, so views read while a scan writes. The queue refers to entries by id, never by position, which is what lets you reorder while the decoder is already reading the next song.

Server songs sit in the same index as local files, under paths like `server://Home/Nirvana/Bleach/01 Blew.flac` that follow the server's folder layout. That's why every view, search and the folder tree work the same for both.

The tests generate their own audio with FFmpeg and run stand-in servers inside the test (Navidrome-like, Gonic-like, Ampache, DLNA, Last.fm, LRCLIB, radio streams and feeds), so they need no internet. They check gapless joins sample for sample, bit-perfect output byte for byte through ALSA's file plugin, leveling, the EQ and limiter against exact figures, and the cache reading a song while it downloads. The servers workflow runs the same clients against real Navidrome, Gonic, Ampache, MiniDLNA and Gerbera containers.

To try it against a server without having one: `cargo run -p patina-core --example mock_server` starts a stand-in Navidrome and prints its address. Log in as `demo`, password `demo`.

To time indexing on your own collection: `cargo run --release -p patina-core --example scan -- ~/Music`. It uses a throwaway database, so your real index isn't touched.

## Limitations

- Bit-perfect needs the DAC to take the file's own format; nothing is converted to make it fit. Going between songs of different formats reopens the card, so there's a short gap. Same-format songs stay gapless.
- With the sample rate on Match, the output reopens between albums at different rates, a short gap on purpose. Set a fixed rate if you'd rather never have it.
- The amp stack costs CPU: about 4% of one core for a typical preset at 4x oversampling, roughly double at 8x.
- A song streaming as a conversion (Opus or MP3) has no known size until it's done, so a seek past what's downloaded waits for the download to get there. The original file seeks straight away.
- One download runs at a time, so the most urgent one gets the whole line. A big pinned library on a slow line takes a while.
- DLNA sources are songs only: DLNA has no playlists, queues, ratings or stars to reach, and no way to tell the server about plays. Finding DLNA servers needs multicast, so one on another subnet or behind a VPN has to go in the config by its description address.
- Navidrome makes its folder tree up from tags (Artist/Album/01 - Title.flac) unless its `Subsonic.DefaultReportRealPath` option is on. Navidrome has no podcasts, and Ampache has no saved play queue and no lyrics call.
- Mixes match Last.fm's answers to your library by artist and title as tagged, apart from case. "The Beatles" and "Beatles" don't match.
- A tag edit on the server that leaves an album's song count, length and dates alone isn't seen by the regular sync. Sync now, in Settings, reads every album and catches it.
- Metered connections are only detected through NetworkManager.
- inotify needs one watch per folder, so a very large library can run past `fs.inotify.max_user_watches`. The app says so if it does, and raising the limit with `sysctl` fixes it.
- On Android, folders are added by path, so a folder from a cloud app's storage provider can't go in. Add that music through a server instead. Bit-perfect depends on the phone's maker offering it. Android Auto has nothing to browse; music is picked on the phone.
- FFmpeg doesn't check certificates on HTTPS radio and podcast streams. Server, Last.fm and lyrics calls go through the app's own HTTP client, which does.
- Crossfeed is a simple bs2b-style crossfeed, not a measured head model. Width, balance and crossfeed only touch stereo; anything with more channels passes through untouched.

## License

GPL-3.0-or-later. See `LICENSE`.

The bundled fonts (Inter, Noto Serif, JetBrains Mono, M PLUS Rounded 1c, Zen Maru Gothic) are under the SIL Open Font License, in `crates/patina-gui/ui/fonts/`.
