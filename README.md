# PatinaPlayer

PatinaPlayer is a music player for Linux and Android that plays your local folders and your Subsonic, Ampache and DLNA servers through one library, for people who want a quiet interface on top of a serious playback chain.

It comes as two apps on one core: a desktop app (Slint) that's also the Android app, and a terminal app (ratatui) that does all the same things from a keyboard. They share the config, the library and every view, so a fix to how something behaves lands in both.

## What it does

### Your music, wherever it lives

- **Local folders**, indexed into one SQLite database, so browsing is instant and works offline. Tags come from lofty (ID3v2, Vorbis comments, MP4, APE), with FFmpeg as the fallback. A rescan only reads new or changed files, and the folders are watched while the app runs, so an album you copy in shows up a couple of seconds later.
- **Subsonic servers** (Navidrome, Gonic, Airsonic, LMS and anything else that speaks the API) and **Ampache**, as many as you like. The app logs in and works out which kind it is, so there's nothing to pick. The server's library is copied into the local index, so its songs sit in every view next to your local music and browsing never waits on the network.
- **Plex Media Server**, through the API Plex documents at developer.plex.tv. Sign in with a code at plex.tv/link and pick your server, or add one by its address and a token. See [Plex](#plex) below.
- **DLNA servers** (MiniDLNA, Gerbera, Jellyfin, Plex, Serviio, Twonky, most NAS boxes). Find DLNA servers asks the network who's there. No address to type, no login.
- **Icecast servers**: give it the address, and every live stream on it plays like a radio station, listed on its page and in Radio, and read again every five minutes so streams that start or stop come and go.
- Each kind has its own page in the sidebar under Sources (Plex, Subsonic & Ampache, DLNA, Icecast, and Snapcast for the rooms you play to), where they're added, synced, edited and removed. Settings keeps the rest.
- Server playlists, play queues (Subsonic), radio stations and podcasts come across too. Changes to a server playlist go back to the server.
- Ratings sync both ways with Subsonic, Ampache and Plex; starred albums and artists with Subsonic and Ampache.
- Passwords live in the desktop keyring (KWallet, GNOME Keyring) through the Secret Service API, never in the config file. With no keyring (a TTY, SSH, a phone) they go in a file only you can read, and the app says so.

### Views

- Folders, Artists (by album artist or track artist, as a grid of pictures or a list), Albums (by name, artist, year, recently added, most played, rating or random), Songs (sortable on seven columns), Genres, Playlists, Radio, Podcasts, Queue and Downloads.
- Search over everything at once. Every word has to match somewhere, so `nirv blea` finds Bleach.
- Recently played: every song you've heard, newest first, under Today, Yesterday, the weekday or the date. Albums and Songs can sort by recently played too.
- Listening stats: Most listened (top five songs, albums and artists), Top albums and Top artists, built from your play counts. A song counts once you've heard half of it or four minutes, whichever comes first.
- Favorites: star a folder, album or artist and it sits in the sidebar, one click away.
- Right-click a song (long press on a phone), in any list or in the queue, for Go to album and Go to artist.
- Pick several rows and act on them at once: play them, queue them, add them to a playlist, keep them offline, send them to an iPod, or take them out of a playlist or the queue. Ctrl-click and Shift-click on the desktop, Space in the terminal app, and on a phone long press, Select, then tap the others. A bar at the bottom says how many are picked.
- Drag songs by the grip at the right of each row to reorder the queue or a playlist. On a touch screen, tap the grip to lift the song and tap where it should go.
- Now Playing: at the top of the sidebar, or tap the cover in the bottom bar (Ctrl+N in the terminal app), for the big cover, the song, its format, whether it's bit-perfect, the output it's going to, every stage of the sound chain that's touching it, and the lyrics beside it. Going anywhere else (the sidebar, a search, Back, Go to album) closes it, so you see where you went. On a phone it takes the whole screen and keeps it simple: the cover, the song with its heart, the seek bar with the format between the times, the transport, volume, and lyrics and the queue at the foot. Rating, Go to album or artist, the cover and the sound chain are in the menu at the top.
- Cover backdrop: the playing song's cover, blurred, washed over whatever theme you use, behind Now Playing or behind every view. Off by default; Settings, Appearance turns it on and sets how strong it is. It works the same on every theme, OLED included (though on OLED it lights pixels that would otherwise be off). Behind every view, the sidebar, the bottom bar, the queue and lyrics panels, the search box and the EQ and amp panels let it through too. A light cover under light text (or a dark one under dark text) gets a veil of the theme's own ground over it, as much as the muted text needs to stay readable and no more.
- Visualizer on Now Playing: click (tap) the cover to step through bars, mirrored bars, an oscilloscope, a ring, and back to just the cover. It draws what's actually going out, after the EQ and the rest of the chain. The choice is kept, and it's also in Settings, Appearance.
- Minimal UX (Settings, Appearance): the bottom bar loses the limiter, EQ, amp, gain and format badges, and the output device becomes a plain button beside cast and the sleep timer (its name in the tooltip). Now Playing loses the format, the output line and the sound chain. Everything still works; it just isn't shown.
- Cover accent: the play button, the seek and volume bars and Now Playing's buttons take a strong color from the playing cover instead of the theme's. A cover with no strong color (black and white, washed out) keeps the theme's. Off by default, in Settings, Appearance.
- Artist pictures, round, in Artists. A local artist's folder can hold one (`artist.jpg`, `band.jpg`, `photo.jpg`, or `folder.jpg` in the folder above the albums); Subsonic and Ampache servers send their own. An artist with no picture anywhere gets a plain mark, and isn't asked about again for a week.
- A section opened for the first time shows placeholder rows while it's read, and covers still loading shimmer, so a big library never looks stuck or empty.
- Covers you choose: right-click an album, a folder, a song, the album header's cover or Now Playing's cover (long press on a phone) for Find cover art or Cover from an image file. Find looks the album up on iTunes, Deezer and MusicBrainz (Cover Art Archive) and shows what it finds to pick from. Either way the picture is saved as `album_art.jpg` (or `.png`) in the album's folder, where other players find it too, and it wins over any art embedded in the files. A server album's folder, or one the app can't write to (most folders on Android), keeps it in the app's own folder instead.
- Hover any icon button for what it does.
- Every library view has Play, Shuffle, Add to queue and Add to playlist at the top, and they take the whole view. Shuffle on a folder of three hundred albums plays all of them in random order.
- Ratings, 1 to 5 stars, on songs and albums.
- Lyrics, only when you press the Lyrics button: the song's tags, an `.lrc` next to it, its server, then LRCLIB if you've turned that on. Synced lyrics follow the song.

### Smart playlists

A smart playlist is a saved search that keeps itself up to date. Make one from Playlists with New smart playlist (`S` in the terminal app).

In the window it opens as rows, like iTunes: Match all or any of the rules, then a row per rule with a field, a condition and a value, and − and + to take a row out or add one under it. Rating rows take stars, ranges take two numbers, Last played and the dates take "in the last 6 months" or a date. Under the rows, Limit to N songs, minutes, hours, MB or GB, and the order they're picked or sorted by (random, highest rating, most recently played, and so on). The count of matching songs updates as you change anything. Edit as text switches to the rules as text, for what rows can't say (groups in brackets, "and" mixed with "or", the leave-out box), and Edit as rules goes back when they fit. The terminal app uses the text.

Every library starts with three by rating: One Star, Mid Tier (2 and 3 stars) and Best Rated (4 and 5 stars). Change or delete them like any other.

As text:

```
rating >= 4 and genre is "blues rock" and played > 180d sort by random limit 100
```

A second box takes songs back out: anything its rules are true for is left out, whatever the first box says. It's often easier than fitting a `not` into the first box. For 60s pop without The Beatles:

```
Songs where:            year = 1960-1969 and genre = pop
Leave out songs where:  artist = "the beatles" or artist = beatles
```

The check under the boxes says how many songs match and how many the second box left out, so a leave-out rule that misses (a misspelled artist) shows as `0 left out`. In the terminal app the second box comes after the first, when you press Enter. Sort and limit go in the first box.

- Fields: `title`, `artist`, `albumartist`, `album`, `genre`, `composer`, `grouping`, `category`, `comment`, `year`, `bpm`, `rating`, `albumrating`, `plays`, `played`, `added`, `modified`, `length`, `track`, `disc`, `format`, `rate`, `bits`, `bitrate` (kbps), `channels`, `size` (MB), `lossless`, `folder`, `source`.
- BPM, composer, grouping, category (the iTunes and podcast one) and comment come from the files' tags. After updating from a version without them, the first scan reads every file's tags again to fill them in, which on a big library takes about as long as the first scan did. Server songs don't have them.
- Text: `is` (or `=`), `isnt` (or `!=`), `contains`, `starts`, `ends`, and `~` for contains. Case doesn't matter. Quote anything with a space in it.
- Numbers: `=`, `!=`, `<`, `<=`, `>`, `>=`, and ranges like `year = 1960-1969` (or just `year 1960-1969`).
- Times are ages: `played > 180d` is not played in six months (or never), `added < 2w` is added in the last two weeks. `h`, `d`, `w`, `mo` and `y` all work, and so do dates like `added > 2024-06`.
- Lengths as `4:30` or `5m`, sample rates as `44.1k`.
- `and`, `or`, `not` and parentheses. Two rules side by side mean `and`.
- A song without a tag is never caught by a rule about that tag, either way: `artist isnt "the beatles"`, `not (artist is "the beatles")` and the leave-out box all keep songs with no artist at all.
- Shortcuts: `played never`, `lossless`, `lossy`, `unrated`.
- At the end, `sort by <field> [asc|desc]` (or `sort by random`) and `limit N`, where N is songs unless it says otherwise: `limit 90 minutes`, `limit 2 hours`, `limit 700 mb`, `limit 4 gb`. A time or size limit takes songs in the playlist's order until the next one wouldn't fit.

The rules look a bit like SQL, but what you type never becomes part of a database statement. The app reads the rules into its own list of conditions first, then builds the query from fixed pieces: field names, operators and sort orders come from lists in the code, and every value you type goes to SQLite as a bound parameter, so it's only ever compared against, never run. `%` and `_` in a value match those characters literally. Anything outside the language is turned away with a message, before the database sees it. Rules are capped at 4000 characters, 200 conditions and 32 levels of nesting, far past anything you'd type, so a pasted wall of text gets a message instead of hanging the app.

### Playlists in and out

- Import M3U, M3U8 and PLS files from Playlists (Import, or `<` in the terminal app). Each entry matches your library by its path, then by its last folders and file name, so a playlist made on another computer with the music somewhere else still lines up. Entries that match nothing are kept as missing songs if they're local paths, so they turn up when the files do. Web addresses are left out, and the import says how many of each there were.
- Export a playlist, a smart playlist, a mix, the queue or any list of songs as M3U8 (Export, or `>`). It goes to a `Playlists` folder in your first library folder unless you say otherwise. Local songs go out as full paths. Server songs go out as their `server://` paths, which only PatinaPlayer reads.

### Playback

- Anything FFmpeg decodes: FLAC, ALAC, WAV, AIFF, APE, WavPack, MP3, AAC, Opus, Vorbis and the rest.
- Gapless, sample-exact. The tests prove FLAC joins match the source and MP3 padding gets trimmed.
- Crossfade from 0 to 12 seconds, never between songs that follow on from each other on an album, so live albums and DJ mixes stay gapless.
- Sample rate set to Match (each album at its own rate) or a fixed rate, with three resampler qualities. Songs on one album that change rate are resampled to keep the join gapless.
- PipeWire output with ALSA fallback on the desktop, AAudio on Android. Pick the output from the bottom bar and the song carries on from where it was.
- Clearing the queue takes away what's coming up. The song playing finishes, then nothing plays.
- Sleep timer: the moon in the bottom bar or on Now Playing (`Z` in the terminal app). Pick 15 minutes to an hour and a half, the end of this song, or type your own: minutes (`45`), a length (`1h30`), or a time of day (`23:30`, `11:30pm`). The volume comes down over the last 30 seconds, then it pauses, and the volume is back for the next play. It runs in the player itself, so it keeps going with the window closed or the phone's screen off. Bit-perfect gets no fade, since that would change the samples; it just pauses. The timer lasts until it runs out or you turn it off, and isn't kept when the app closes.

### The sound chain

In order, after decoding: leveling, the amp stack, the EQ, the channel tools, volume, then the limiter.

- **Volume leveling**, the ReplayGain 2 way, to -18 LUFS by default. Smart mode uses the album gain while an album plays in order and the track gain on shuffle and in mixes. Gains come from the server, then the file's tags, then the app's own EBU R128 measurement, made once per song and kept. Radio and podcasts get a live normalizer instead.
- **Amp stack**: a rack of old hardware you chain in any order. Tube preamp, tube power amp, output transformer, tape deck, phono stage, console channel, bus compressor, exciter and bass enhancer, each with its own knobs, oversampled 4x by default. Off by default and subtle at its defaults. They're character effects built from how each kind of hardware behaves, not models of branded units.
- **EQ**: graphic (10, 15 or 31 bands) or parametric (up to 20 bands of peak, shelf, pass and notch filters). Imports AutoEq and Equalizer APO files. Presets, built-in and your own. Each output device can keep its own EQ, and on PipeWire it switches with the default output, mid-song.
- **Channel tools**: mono, stereo width, crossfeed for headphones (bs2b style) and balance.
- **True-peak limiter**, last in the chain, at -1 dBTP by default, so EQ boosts and leveling never clip. Below the ceiling it doesn't touch a sample.
- **Bit-perfect**, off by default. Local files and original server files go straight to the DAC, opened exclusively, at the file's own rate and depth, with nothing in between. On a PipeWire desktop it asks PipeWire to let go of the card first. If the DAC can't take the file as it is, it says why and plays through the normal output instead. It never resamples quietly. Auto mode turns it on only when nothing else would change the sound.

Everything in the chain glides when you change it, so nothing clicks or zippers.

### Plex

Plex is a server kind like the others: its music libraries are copied into the index, and its songs sit in every view next to your own.

- **Signing in.** On the Plex page, Sign in shows a four-character code. Enter it at plex.tv/link on any device, signed in to Plex there. The app never sees your Plex password. It then lists the servers on your account (your own and ones shared with you) to add. Following Plex's docs, the app makes its own Ed25519 key, sends the public half with the code, and gets a Plex JWT back. That token lasts 7 days, and the app swaps it for a new one a day before it runs out, signing with the key. Plex allows that even after it has run out, so being away for a month is fine. The key and tokens live in the keyring. Add by hand takes a server address and a classic X-Plex-Token instead, without plex.tv.
- **Finding the server.** plex.tv lists every address the server has. The app tries the one that worked last, then the local ones, then the internet ones, then Plex's relay, and keeps the first that answers as that server.
- **Playing.** Songs stream as the original file, so bit-perfect and gapless work as with local files. Set a server's streaming to a conversion and Plex's own transcoder makes it (AAC, or MP3 for the MP3 choices), which the cache downloads and keeps like any other.
- **Ratings** go both ways (Plex counts out of ten, so a star is two).
- **Plays and Now Playing.** While a song plays, the app tells Plex where it is every ten seconds and whenever it pauses, resumes or stops, the way Plex's own players do. That puts PatinaPlayer on Plex's dashboard, and Plex counts the play once the song gets near its end, so "most played" and history in Plex stay right.
- **Playlists** come across, and changes to yours go back. Plex's smart playlists come across too, read-only, and are read again every half hour since their songs change on their own.
- **Plex Pass features**, a switch per server, off by default: mixes from Plex's sonic analysis (songs that sound alike, rather than Last.fm's or genre tags' idea of alike), lyrics from Plex, and loudness from Plex's analysis, so leveling doesn't have to measure every song. They need Plex Pass on the server's owner's account; the row says when the server doesn't have it. With the switch off, or no Plex Pass, mixes, lyrics and leveling work the way they do for every other server.

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

### Last.fm and ListenBrainz

- Link your Last.fm account from Settings. Now Playing, scrobbles by Last.fm's rules, and love (the heart) that's kept in the library and sent to Last.fm.
- ListenBrainz runs alongside it, or on its own. Paste your user token from listenbrainz.org/settings into Settings, ListenBrainz. It's checked, and kept in your keyring. Listens and Now Playing go out by the same rules as Last.fm scrobbles. A self-hosted ListenBrainz, or anything else with the same API (Maloja, Koito), works by setting its address.
- Scrobbles and listens made offline wait in the library and go out later with their original times.
- For each server you choose who scrobbles, separately for Last.fm and ListenBrainz: the app, the server, or both. Navidrome and others pass plays on themselves, so picking the right one keeps plays from counting twice.

### iPod

Plug in a classic iPod, a mini, or an older nano and it shows up in the sidebar under Sources once your desktop mounts it. The iPod view lists the playlists and albums on it, with how much space is left.

- Send to iPod (right-click, or Ctrl+P in the terminal app) takes the selection, the picked rows, or the whole view. A playlist, smart playlist or mix arrives as a playlist of the same name on the iPod. Send Favorites puts everything you've starred on it.
- Songs the iPod can't play (FLAC, Opus, Vorbis, anything above 48 kHz or 16 bits) are converted on the way. Settings, iPod picks what to: Apple Lossless at 16 bits (the default, with lossy songs going to AAC), AAC 256 kbps, or MP3 320 kbps. MP3, AAC and ALAC it can play go across untouched. Covers go too.
- Songs already on it (same artist, album, title and track) aren't sent twice.
- Take a playlist or an album off it, and eject it when you're done, from the same view.

It needs libgpod installed, which is what Rhythmbox and gtkpod use (`libgpod4` on Debian and Ubuntu, `libgpod` on Fedora, Arch and openSUSE), and the `ffmpeg` command for converting. Without libgpod the iPod view says so and nothing else changes.

### Radio and podcasts

- Internet radio: any stream FFmpeg can decode, Icecast and SHOUTcast included. `.pls`, `.m3u` and `.m3u8` links resolve on their own, HLS goes straight to FFmpeg, and a dropped stream reconnects with backoff. Test a station and it tells you what the URL turned out to be.
- Podcasts: subscribe by feed URL, pick up where you left off, 0.5x to 3x speed with the pitch kept, silence trimming, and downloads for offline. While an episode plays, the speed chip on the bottom bar steps through 1x, 1.2x, 1.5x and 2x on a click, and right-click (or a long press) opens all the speeds and Skip silence. On a phone the chip sits on Now Playing beside the format; tap it for the same menu.

### Other devices

- **Play on** another device: the cast button on the bottom bar or on Now Playing, or the last entry in the output picker. It lists Chromecasts, DLNA renderers (network speakers, AV receivers, TVs, Kodi) and Snapcast servers found on the network. The sound still comes from this app's player, after the EQ, the amp stack and leveling, so the queue, gapless and crossfades all carry on. Chromecasts and DLNA renderers fetch it from the app as a live 48 kHz WAV stream; a snapserver gets it over TCP. Pick This device to come back.
- For Snapcast, nothing has to change on the snapserver. When you pick it, PatinaPlayer asks the snapserver over its control API (port 1705, or 1780) for a TCP source on port 4953 and adds one called "Patina" if there isn't one, then checks it can connect, and says what's wrong if it can't. Then turn rooms on from the Snapcast page (below), or set them to the Patina stream in Snapweb or the Snapcast app. A source added that way lasts until the snapserver restarts, and PatinaPlayer adds it again the next time. To keep it for good, put `source = tcp://0.0.0.0:4953?name=Patina&mode=server` in the `[stream]` section of the `snapserver.conf` the snapserver actually reads (in Docker, the one mounted into the container), and publish port 4953 if it runs in Docker. A snapserver that doesn't announce itself goes in by address, from the same list or the Snapcast page.
- **Snapcast rooms**: the Snapcast page lists the snapclients the snapserver knows. Turn one on and PatinaPlayer plays in that room, with nothing else changing: a room that shares a Snapcast group with others is moved into a group of its own first, so the others keep what they were playing. Turning it off puts it back on the stream it had. Each room has its own volume there too. Turning the first room on starts playing on that snapserver if PatinaPlayer isn't already. It all goes through the snapserver's control API (`Group.SetClients`, `Group.SetStream`, `Client.SetVolume`).
- **Remote control**: turn it on in Settings, Other devices, and PatinaPlayer on a phone (or another computer) on the same network can steer this one: the song, its cover, play, pause, skip, seek, volume, shuffle, repeat, the heart, and the queue, where tapping a song plays it. It asks for the six-digit code shown there; New code makes a fresh one and shuts out whoever had the old one. On the other side, Settings, Other devices, Control another player lists the players on the network (or takes an address) and opens their Now Playing. Back or Escape leaves it. The code from last time is kept, so the second time it's one tap.

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
- Android Auto works with the app closed on the phone: connecting the car starts the player in the background, with your last queue where you left it, and opening the app later carries on with the same player. The car shows what's playing with next, previous and play/pause, plus a heart for the song playing and a shuffle button. Its tabs are Home (Shuffle everything, Favorites, Recently played, Recently added, Mixes), Artists, Albums, Playlists (smart playlists included) and Folders, with album covers in the lists and as grids. A long list of artists or albums comes as one folder per letter. A car with room for only four tabs gets Home at the top of Playlists. "Play Nirvana on PatinaPlayer" plays an artist, album or songs that match.
- A home screen widget with the cover, the song, and back, play or pause, and next, in the app's theme. It keeps the theme after a restart.
- The System theme (Android 12 and later) follows your wallpaper's Material You colors and the phone's light or dark mode.
- The volume button opens a tall slider, easier to hit with a thumb than the bar's.
- Minimal UX while driving (Settings, Appearance, on by default): when the phone goes into car mode, which Android Auto turns on and so do driving modes built on it, the app switches to Minimal UX by itself, and back after.
- Gestures: swipe up on the bottom bar to open Now Playing, swipe down anywhere on Now Playing to close it (the page follows your finger), and swipe the cover left or right for the next or previous song.
- Back (the edge swipe or the button) closes what's open first (a dialog, the sidebar, a panel, Now Playing), then goes up a level, from an album back to its artist. At the top, a swipe in from the left edge pulls the sidebar out; from the right, or the button, leaves the app with the music playing. Telling the edges apart needs Android 14; on 13 both edges do what the right does.
- Android Auto has a queue screen: what's coming up, with covers, and tapping a song plays it.
- Bit-perfect to a USB DAC on Android 14 and later, where the phone's maker offers it.

## Getting it

### The AppImage

Download `PatinaPlayer-<version>-x86_64.AppImage` from Releases.

1. Make it executable: `chmod +x PatinaPlayer-*-x86_64.AppImage`
2. Run it: `./PatinaPlayer-*-x86_64.AppImage`
3. For the terminal app, run it with `--tui`, or symlink it as `patinaplayer-tui` somewhere on your PATH, like `ln -s ~/Applications/PatinaPlayer-0.1.0-x86_64.AppImage ~/.local/bin/patinaplayer-tui`.
4. For a menu entry, use Gear Lever or AppImageLauncher, or whichever AppImage manager you like. The desktop file and icon are inside, and they can update it from the `.zsync` file on the release.

IT NEEDS GLIBC 2.39 OR NEWER, since it's built on Ubuntu 24.04. Kubuntu 24.04, Fedora 40, Debian 13 and later are fine. Older systems get a glibc error and nothing else.

FFmpeg is built into it, audio only, so your distro's FFmpeg doesn't matter. PipeWire, ALSA, GnuTLS, OpenGL, fontconfig and glibc come from your system. So do libgpod and the `ffmpeg` command, which only the iPod sync needs.

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
- Optional, for the iPod sync and its test: libgpod (`libgpod4` on Ubuntu). It's loaded when an iPod turns up, so the build doesn't need it.

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
4. For a menu entry, copy `assets/patinaplayer.desktop` to `~/.local/share/applications/` and `assets/patinaplayer.svg` to `~/.local/share/icons/hicolor/scalable/apps/`, since that's where the desktop file looks for its icon. KEEP THE DESKTOP FILE NAMED `patinaplayer.desktop`: on Wayland, the taskbar finds the icon by matching that name to the name the window gives.
5. Optionally, run the tests: `cargo test --workspace`

### The AppImage, locally

`bash packaging/build-appimage.sh`. It needs everything above plus `nasm`, `libgnutls28-dev` and `zlib1g-dev` (or whichever names your distro uses), since it builds its own FFmpeg. The result lands in `target/appimage/`.

### The APK, locally

`bash packaging/build-apk.sh`. It needs the Android SDK with build-tools and a platform of 34 or newer (`ANDROID_HOME`), the NDK (`ANDROID_NDK_ROOT`), JDK 17 or newer, cmake, and `rustup target add aarch64-linux-android`. It builds FFmpeg and Mbed TLS for Android first, then the Rust library, the Java side and the APK, without Gradle. The result lands in `target/apk/`.

Without a release key in the environment it signs with a debug key kept in `target/android-debug.keystore`, which is fine for your own phone. The script's header lists the four signing variables.

## Making a release

The Release workflow builds the AppImage and the APK from a tag and attaches them to that tag's release. It only runs when you start it, never on a push.

1. Go to Releases on GitHub and click Draft a new release.
2. Under Choose a tag, type a new tag (like `v1.2.0`) and create it there, on `main`.
3. Write the notes and click Publish release.
4. Go to Actions and click the Release workflow.
5. Click Run workflow, pick that tag under "Use workflow from", and run it.
6. Wait. The first run takes about 15 minutes; later ones are faster, since FFmpeg and the Rust build are cached. The AppImage, its `.zsync` and the APK land on the release you made.

If it was run on a branch, or there's no release for the tag yet, it stops in the first few seconds and says which. Running it again on the same tag replaces the files on the release.

THE APK HAS TO BE SIGNED WITH THE SAME KEY EVERY RELEASE, or Android refuses to install it over the old one and the app's data is lost to a reinstall. Put the keystore and its passwords in the repository's secrets once (`ANDROID_KEYSTORE_BASE64`, `ANDROID_KEYSTORE_PASSWORD`, `ANDROID_KEY_ALIAS`, `ANDROID_KEY_PASSWORD`). Until they're there, the APK build fails on purpose and the AppImage still ships.

Every APK on a release has a signed SLSA build provenance attestation: a record, signed through Sigstore, that this workflow built it from that commit. They're listed on the repository's Attestations page, under the Actions tab's sidebar. This works as is for a public repository; a private one needs GitHub Enterprise Cloud for it.

A Last.fm API key and secret can be built in from the `LASTFM_KEY` and `LASTFM_SECRET` secrets. Without them, scrobbling needs your own key in Settings (free from last.fm/api/account/create).

## First run

Both apps work the same here. They share the config and the library, though you'd only run one at a time.

1. Run it once: `patinaplayer` (or `patinaplayer-tui`). That writes `~/.config/patinaplayer/config.toml`, with your Music folder (`xdg-user-dir MUSIC`) already in it if your system has one.
2. Add other folders in Settings, under Library: Browse for one, or type its path. The scan starts right away, with progress next to the folder in the sidebar.

From the command line, if you'd rather: `patinaplayer --add-folder /mnt/storage/music` adds one and exits, and `--remove-folder` takes one out.

To add a Plex server:

1. Open Plex in the sidebar, under Sources.
2. Choose Sign in. A code shows up.
3. On any device, go to plex.tv/link, sign in to Plex, and enter the code. The desktop app has a button that opens the page.
4. Pick the server. One server on the account is added straight away. The first sync starts, with progress next to it on its page.
5. For Plex Pass features (sonic mixes, lyrics, loudness), turn on its Plex Pass features row.

To add a Subsonic or Ampache server:

1. Open Subsonic & Ampache in the sidebar, under Sources.
2. Choose Add server…
3. Give it a name for the sidebar, its address (whichever address you open its web page at, like `https://music.example.com`), your user name and your password. For an OpenSubsonic API key, leave the user name empty and put the key in the password box.
4. Connect. If the login works it's saved and the first sync starts, with progress next to its name in the sidebar.

To add a DLNA server:

1. Make sure it's on and on the same network as you.
2. Open DLNA in the sidebar, under Sources, and choose Find servers… It listens for about three seconds.
3. Pick the server from the list. It's added under its own name and the sync starts.

If nothing answers, a firewall dropping incoming UDP is the usual cause, since the answers come back to a random port. Let replies from the server's address in through ufw, or whichever firewall you run.

To add an Icecast server: open Icecast under Sources, choose Add server, and give it the address, like `radio.example.com:8000`. It needs Icecast 2.4 or newer, which has the status page the streams are read from.

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
backdrop = "off"         # off, now-playing or everywhere: the playing cover, blurred
backdrop_strength = 60   # 10 to 100 percent
cover_accent = false     # color the transport from the playing cover
minimal = false          # leave the technical readouts off the bar and Now Playing
minimal_driving = true   # Android: Minimal UX on its own in car mode
visualizer = "off"       # off, bars, mirror, scope or ring; clicking the cover steps through

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

[cast]
snapcast = ""            # a snapserver that doesn't announce itself: "host" or "host:port"

[remote]
enabled = false          # let other players on the network control this one
port = 7373
code = ""                # made up when you first turn it on
control = ""             # the player this one last controlled, and its code
control_code = ""

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

[listenbrainz]
scrobble = true          # the token itself is in your keyring, set from Settings
user = ""                # the account the token belongs to; Settings fills it in
url = ""                 # another server with the same API; empty is api.listenbrainz.org

[listenbrainz.servers]   # app, server or both, like [lastfm.servers]
Home = "app"

[ipod]
target = "alac"          # what songs it can't play become: alac, aac or mp3

[[radio.station]]
name = "Some Station"
url = "https://example.com/listen.pls"

[[server]]
name = "Home"            # shown in the sidebar
url = "https://music.example.com"
user = "me"
kind = "subsonic"        # subsonic, ampache, plex, dlna or icecast; adding one sets it
stream = "raw"           # raw (the original file), opus or mp3
bitrate = 192            # kbps, for opus and mp3
# plex_pass = false      # a Plex server's Plex Pass features; udn is its machine id
```

The app writes the `[eq]` and `[amp]` tables itself, along with which device gets which preset. The EQ and Amp views are the easier way in. Command line flags (`--backend`, `--device`, `--buffer-ms`, `--volume`, `--fade-ms`) override the file for one run.

Where everything else goes:

| What | Where |
|---|---|
| Settings | `~/.config/patinaplayer/config.toml` |
| EQ and amp stack presets | `~/.config/patinaplayer/presets/eq/` and `presets/amp/` |
| Library index, playlists, smart playlists, play counts, listening history, ratings, podcast positions, loudness, waiting scrobbles and listens | `~/.local/share/patinaplayer/library.db` |
| Songs kept offline, downloaded episodes | `~/.local/share/patinaplayer/offline/` |
| Cached and prefetched songs | `~/.cache/patinaplayer/tracks/` |
| Cover art at the sizes the app draws | `~/.cache/patinaplayer/art/` |
| Artist pictures, and which artists have none | `~/.cache/patinaplayer/artists/` |
| Server passwords, the Last.fm session, the ListenBrainz token | your keyring, or `~/.config/patinaplayer/secrets.toml` (only you can read it) when there's no keyring |

The index and the offline songs live in `~/.local/share` on purpose, since cleaners like BleachBit wipe `~/.cache`, and nobody should lose their playlists or a pinned library to that. The two caches are safe to wipe. They refill as you play.

On Android all of it lives in the app's private storage.

## Keys

These are the terminal app's keys. The desktop app uses the same ones, except there's no Tab focus switching, `Delete` removes from the queue or the open playlist, `Ctrl+F` also searches, `Alt+Left` and `Alt+Right` go back and forward, Ctrl-click and Shift-click (or Shift+Up and Shift+Down) pick rows, Space always pauses, and the per-view letter commands live in the right-click menu and the buttons at the top of each view. `?` lists them in either app.

HEADS UP IF YOU'VE USED THE TERMINAL APP BEFORE: Space in a list now picks the row instead of pausing. `c` pauses from anywhere, and so does Ctrl+Space or Space in the sidebar.

| Key | Action |
|---|---|
| c, Ctrl+Space | Play / pause (Space too, outside a list) |
| Space | In a list: pick the row and move down |
| U | Pick every row in the view |
| Esc | Drop the picks, or back up a level when nothing's picked |
| Enter | With rows picked, play just those |
| n / b | Next / back (back restarts the song if you're more than 3 seconds in) |
| Left / Right | Seek 5 s, Shift for 30 s |
| + / -, m | Volume, mute |
| s, r | Shuffle, cycle repeat |
| 1 to 9, 0 | Folders, Artists, Albums, Songs, Genres, Playlists, Radio, Podcasts, Queue, Downloads |
| Tab | Switch between the sidebar and the list |
| / | Search |
| Enter | Open, or play the list from the selected song |
| Backspace | Back up a level |
| v / V | Play the whole view in order / shuffled |
| e | Add the whole view to the queue |
| P | Add the whole view to a playlist |
| a / A | Add to the queue / play next |
| i / I | Go to the selected song's album / artist (in the Queue too) |
| p | Add to a playlist (the picked rows, if any) |
| H | Recently played |
| Ctrl+N | Now Playing |
| Ctrl+P | Send the selection, the picked rows or the whole view to the iPod |
| < / > | Import a playlist file / export the selection or view as M3U8 |
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
| Z | Sleep timer: a length (45, 1h30), a time (23:30) or end; empty turns it off |
| . | Jump to the playing song in Queue |
| Ctrl+E, Ctrl+A | The EQ, the amp stack |
| Ctrl+O | Pick the output device |
| Ctrl+L | Lyrics for the song playing |
| F5 | Check folders for changes and refresh podcast feeds |
| Ctrl+Q | Quit |

In Radio: `N` new station, `E` edit, `T` test, `D` delete, `J`/`K` move. In Podcasts: `N` subscribe, `D` unsubscribe; in a show, `w` downloads, `x` deletes the download, `P` marks played. In the Queue: `d` remove (the picked songs, if any), `J`/`K` move, `S` save to a playlist (the picked songs, if any), `L` load the queue saved on your server, `C` clear. In Playlists: `N` new, `S` new smart playlist, `R` rename, `D` delete, `E` edit a smart playlist's rules. In Recently played: the Clear the history row at the top empties it. In the iPod view: `d` take a playlist or album off, `E` eject. In Downloads: `w` pauses or resumes, `x` cancels or unpins, `W` pauses everything, `C` clears the cache. In Settings and the EQ: Enter flips, cycles or edits, `h`/`l` step a value down or up. On the Sources pages the same, plus `E` renames or edits a server and `d` removes it; Sign in shows the Plex code in the message line, and adds your own server once it's entered (sign in again for another on the same account).

TUI cover art picks a protocol by asking the terminal. To force one: `patinaplayer-tui --art kitty` (or `sixel`, `iterm2`, `blocks`, `off`). Konsole supports kitty images. Inside tmux, auto usually falls back to blocks.

## How it's put together

```
crates/
  patina-core/      everything that isn't drawing: audio, library, servers, cache, DSP, views
  patina-gui/       the desktop app (Slint), and the window the Android app shows
  patina-tui/       the terminal app (ratatui)
  patina-android/   the Android app's native library and its JNI glue
android/            manifest, icons and the Java side (permissions, media session, Android Auto, devices)
assets/             app icon (SVG sources and their PNGs) and desktop file
packaging/          AppImage and APK builds, FFmpeg for each
.github/workflows/  tests on every push, real servers in containers, releases when you run them on a tag
```

The core owns all the state, and every view and what each action does lives in `patina-core/src/browse`, so both frontends only draw and take input. The library runs on its own thread with SQLite in WAL mode, so views read while a scan writes. The queue refers to entries by id, never by position, which is what lets you reorder while the decoder is already reading the next song.

Server songs sit in the same index as local files, under paths like `server://Home/Nirvana/Bleach/01 Blew.flac` that follow the server's folder layout. That's why every view, search and the folder tree work the same for both.

The tests generate their own audio with FFmpeg and run stand-in servers inside the test (Navidrome-like, Gonic-like, Ampache, Plex and plex.tv, DLNA, Icecast, Last.fm, LRCLIB, radio streams and feeds), so they need no internet. They check gapless joins sample for sample, bit-perfect output byte for byte through ALSA's file plugin, leveling, the EQ and limiter against exact figures, and the cache reading a song while it downloads. The servers workflow runs the same clients against real Navidrome, Gonic, Ampache, MiniDLNA and Gerbera containers.

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
- A tag edit on the server that leaves an album's song count, length and dates alone isn't seen by the regular sync. Sync now, on its Sources page, reads every album and catches it.
- Plex is built from Plex's published API and tested against a stand-in that follows it, not yet against a real Plex Media Server. Some developers report that a server token plex.tv hands out for a Plex JWT gets refused by their server; the app then tries the account's token, and if that's refused too, Add by hand with a classic token works. Plex has no favorites, so stars don't sync with it, and no saved play queue. Plex's podcasts and its Live TV aren't music libraries and don't come across.
- A Plex conversion comes as HLS segments (the only way Plex's docs offer it), joined into one file as it downloads, so like any conversion it seeks only as far as what's downloaded.
- Icecast needs 2.4 or newer for its JSON status page. A stream that needs a password to listen won't play.
- Metered connections are only detected through NetworkManager.
- inotify needs one watch per folder, so a very large library can run past `fs.inotify.max_user_watches`. The app says so if it does, and raising the limit with `sysctl` fixes it.
- On Android, folders are added by path, so a folder from a cloud app's storage provider can't go in. Add that music through a server instead. Bit-perfect depends on the phone's maker offering it.
- Covers are found by the album's tags, so an album tagged wrong turns up wrong covers or none. Fix the tags, or use an image file. Android Auto has been tried in one car so far, and its queue screen not yet in a car. The heart and shuffle icons come from Android Auto itself on newer versions; an older version draws the app's own.
- The home screen widget's buttons control the player while the app is running. After the phone restarts, or Android stops the app, they open it instead, since there's no player to tell until it starts. With the System theme, the widget picks up a new wallpaper or a switch between light and dark mode the next time the app runs, since only the app can work out the colors.
- The iPod sync covers what libgpod covers: iPod classic, the older nanos (up to the 5th generation), minis, and shuffles up to the 2nd generation. The 6th and 7th generation nanos and the iPod touch don't work. It's desktop only. Play counts and ratings made on the iPod don't come back.
- Smart playlists are typed rules, not a form of drop-downs. The dialog checks them as you type and the examples are one click away, but it's still a small language to learn.
- FFmpeg doesn't check certificates on HTTPS radio and podcast streams. Server, Last.fm and lyrics calls go through the app's own HTTP client, which does.
- Playing on a Chromecast or a DLNA renderer runs a few seconds behind: they buffer before they play, so pause, skip and the volume take that long to be heard, and the position shown runs ahead of what you hear. Snapcast is as late as its own buffer, a second by default. The app's player keeps time, so a song that ends here moves on there too. A Chromecast shows "PatinaPlayer" rather than the song. Some DLNA renderers don't play a live WAV stream at all.
- The remote control is plain HTTP on your network. The code keeps out the curious, not someone watching the network, and it sits in `config.toml` as written. Finding players and devices needs multicast (mDNS, SSDP), like finding DLNA servers.
- Crossfeed is a simple bs2b-style crossfeed, not a measured head model. Width, balance and crossfeed only touch stereo; anything with more channels passes through untouched.

## License

GPL-3.0-or-later. See `LICENSE`.

The bundled fonts (Inter, Noto Serif, JetBrains Mono, M PLUS Rounded 1c, Zen Maru Gothic) are under the SIL Open Font License, in `crates/patina-gui/ui/fonts/`.
