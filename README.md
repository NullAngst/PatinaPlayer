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
- Each kind has its own page in the sidebar under Sources (Plex, Subsonic & Ampache, DLNA, Icecast), where they're added, synced, edited and removed. Snapcast is where music goes, not where it comes from, so its page sits under Play on. Settings keeps the rest.
- **Snapcast streams**, listened to here: the Snapcast page lists the snapserver's streams, and Listen here plays one on this device as a station. See [Other devices](#other-devices).
- **One source or all of them.** The picker at the top of the sidebar (in the drawer on a phone) picks what Artists, Albums, Songs, Genres, Folders, search, smart playlists and mixes show: All sources, Local, or one server. The queue and your playlists always show everything. With All sources, an album more than one source has shows once, from the first source in Settings, Library, Source order (local first, then Plex, then the rest, unless you change it). When that source can't be reached, the album shows from the next one that has it. Mixes keep every copy and play whichever can play. Android Auto shows the same source. In the terminal app `B` steps through them and the status bar says which.
- Server playlists, play queues (Subsonic), radio stations and podcasts come across too. Changes to a server playlist go back to the server.
- Ratings sync both ways with Subsonic, Ampache and Plex; starred albums and artists with Subsonic and Ampache.
- Passwords live in the desktop keyring (KWallet, GNOME Keyring) through the Secret Service API, never in the config file. With no keyring (a TTY, SSH, a phone) they go in a file only you can read, and the app says so.

### Views

- Folders, Artists (by album artist or track artist, as a grid of pictures or a list), Albums (by name, artist, year, recently added, most played, rating or random), Songs (sortable on seven columns), Genres, Playlists, Radio, Podcasts, Queue and Downloads.
- Search over everything at once. Every word has to match somewhere, so `nirv blea` finds Bleach. Your last eight searches drop down under the empty search box (on Android, under Android's own search box); pick one to search for it again, or Clear them. They're kept in `recent-searches.txt` in the app's data folder.
- A letter scrubber down the right edge of a long list in A to Z order (Artists, Albums, Songs, Genres, Folders, a server's folders): press or drag on it and the list jumps to that letter, with the letter shown big beside your finger. It works out which column the list is sorted by (the title, the artist or the album) and only shows when the list is in that order, so a list by year or a playlist's own order gets none.
- Going back to a view puts it back where you'd scrolled to, from an album back to a long artist list, say. Opening a view from the sidebar starts it at the top.
- Refresh, in the buttons of Artists, Albums, Songs, Genres, Folders, Playlists and Podcasts (in the "…" menu on a phone), looks again: the server picked in the sidebar (or all of them) syncs, and your local folders get checked. On Podcasts it fetches the feeds.
- A list leaves out a column every row has the same thing in, or nothing: a playlist of one artist's songs has no artist column, and that artist goes under the playlist's title instead. The title gets the room. An empty page says what fills it (Hearted songs says where the heart is).
- Recently played: every song you've heard, newest first, under Today, Yesterday, the weekday or the date. Albums and Songs can sort by recently played too.
- Listening stats: Most listened (top five songs, albums and artists), Top albums and Top artists, built from your play counts. A song counts once you've heard half of it or four minutes, whichever comes first.
- Favorites: star a folder, album or artist and it sits in the sidebar, one click away.
- Right-click a song, in any list or in the queue, for its menu. Go to album and Go to artist sit in a Go to submenu, and Find online and From an image file in a Cover art (or Artist picture) submenu, so the menu stays short.
- On a touch screen, a long press opens a sheet from the bottom instead of a menu: the song or album at the top, big buttons for Play, Play next, Queue and Playlist, the stars for a song or an album, then the rest of the menu as full-width rows a thumb can hit. Back or a tap outside closes it. The window works out which you're using from the last press: a finger gets the sheet, the bigger rows and the touch drag handles, a mouse gets the menu, so a touchscreen laptop or a phone with a mouse gets the right one each time.
- View info, in the same menu for a song or an album (and in the queue): everything known about it in a panel where the lyrics go. For a song: title, artists, album, track and disc, year, every genre, composer, grouping, BPM, comment, length, rating, the heart, plays, last played, when it was added, the format in full, the file's size and path (or its server and id), and ReplayGain. Below that, every tag in the file as it is right now, and the pictures in it, for a song on this device or a server song kept in the cache. For an album: its artists, years, genres, songs, discs, length, rating, plays, the formats it comes in, its size and its folder. The Info button on Now Playing opens it for the song playing, and then it moves on with the queue. Tap or click a line to copy its value (a path, a MusicBrainz id, the format), and the note says what was copied.
- Lyrics, Info and the Queue share one panel at the right. Open more than one and they become tabs across its top, so they never squeeze the list between them. Ctrl+L or `q` while that panel is open behind another tab brings it to the front instead of closing it.
- Undo instead of "Are you sure?": deleting a playlist, a smart playlist, a saved mix or a radio station, clearing the queue, and taking songs out of a playlist or the queue happen at once, and the note that says so has an Undo button for 10 seconds (Ctrl+Z on the desktop and in the terminal app). Tap the note to take it down; it also goes when you leave that page. Only the last one can be put back. A playlist on a server still asks first, since the server deletes it for good. A mix that was kept offline comes back without its offline copies; keep it offline again.
- Pick several rows and act on them at once: play them, queue them, add them to a playlist, keep them offline, send them to an iPod, or take them out of a playlist or the queue. Ctrl-click and Shift-click on the desktop, Space in the terminal app, and on a phone long press, Select, then tap the others. A bar at the bottom says how many are picked, with Pick none and Invert (everything not picked, and the picked ones dropped). The list runs on under the bar, with room after its last row, so the last rows scroll out from under it.
- Drag songs by the grip at the right of each row to reorder the queue or a playlist. On a touch screen, tap the grip to lift the song and tap where it should go.
- Now Playing: at the top of the sidebar, or tap the cover in the bottom bar (Ctrl+N in the terminal app), for the big cover, the song, its format, whether it's bit-perfect, the output it's going to, every stage of the sound chain that's touching it, and the lyrics beside it. Going anywhere else (the sidebar, a search, Back, Go to album) closes it, so you see where you went. On a phone it takes the whole screen and keeps it simple: the cover, the song with its heart, the seek bar with the format between the times, the transport, volume, and lyrics and the queue at the foot. Rating, Go to album or artist, the cover and the sound chain are in the menu at the top.
- Cover backdrop: the playing song's cover, blurred, washed over whatever theme you use, behind Now Playing or behind every view. Off by default; Settings, Appearance turns it on and sets how strong it is. It works the same on every theme, OLED included (though on OLED it lights pixels that would otherwise be off). Behind every view, the sidebar, the bottom bar, the queue and lyrics panels, the search box and the EQ and amp panels let it through too. On a phone, where the lyrics, info and queue panel covers the list, the panel is solid with the backdrop drawn into it, so the list doesn't show through. A light cover under light text (or a dark one under dark text) gets a veil of the theme's own ground over it, as much as the muted text needs to stay readable and no more.
- Visualizer on Now Playing: click (tap) the cover to step through bars, mirrored bars, an oscilloscope, a ring, and back to just the cover. It draws what's actually going out, after the EQ and the rest of the chain. The choice is kept, and it's also in Settings, Appearance.
- Minimal UX (Settings, Appearance): the bottom bar loses the limiter, EQ, amp, gain and format badges, and the output device becomes a plain button beside cast and the sleep timer (its name in the tooltip). Now Playing loses the format, the output line and the sound chain. Everything still works; it just isn't shown.
- Cover accent: the play button, the seek and volume bars and Now Playing's buttons take a strong color from the playing cover instead of the theme's. A cover with no strong color (black and white, washed out) keeps the theme's. Off by default, in Settings, Appearance.
- Artist pictures, round, in Artists. A local artist's folder can hold one (`artist.jpg`, `band.jpg`, `photo.jpg`, or `folder.jpg` in the folder above the albums); Subsonic and Ampache servers send their own. An artist with no picture anywhere gets a plain mark, and isn't asked about again for a week.
- Artist pictures you choose: right-click an artist (long press on a phone) for Find artist picture, which looks the name up on Deezer (the one open source of artist pictures; iTunes and MusicBrainz have none), or Artist picture from an image file. It's saved as `artist.jpg` in the folder all the artist's albums share, where other players find it too, or in the app's own folder for an artist on a server or whose albums don't share a folder it can write to. Either way it wins over everything else.
- A section opened for the first time shows placeholder rows while it's read, and covers still loading shimmer, so a big library never looks stuck or empty.
- Covers you choose: right-click an album, a folder, a song, the album header's cover or Now Playing's cover (long press on a phone) for Find cover art or Cover from an image file. Find looks the album up on iTunes, Deezer and MusicBrainz (Cover Art Archive) and shows what it finds to pick from. Artist, album and year fields sit at the top of that box on the desktop, filled in from the tags; change them and press Search (or Enter) to look again when the tags are wrong or too long (a "Deluxe Edition" the stores don't call it). On a phone, Search for asks for them in Android's dialogs. The year narrows MusicBrainz to that year's release and puts covers from that year first. Either way the picture is saved as `album_art.jpg` (or `.png`) in the album's folder, where other players find it too, and it wins over any art embedded in the files. A server album's folder, or one the app can't write to (most folders on Android), keeps it in the app's own folder instead.
- Hover any icon button for what it does.
- Every library view has Play, Shuffle, Add to queue and Add to playlist at the top, and they take the whole view. Shuffle on a folder of three hundred albums plays all of them in random order.
- Ratings, 1 to 5 stars, on songs and albums.
- Lyrics, only when you press the Lyrics button: the song's tags, an `.lrc` next to it, its server, then LRCLIB if you've turned that on. Synced lyrics follow the song, with the line playing kept in the middle of the panel, larger, and the others dimmed (Settings, Lyrics turns that off for the old way, from a couple of lines before it at the top). Plain lyrics leave out LRC's header tags (`[ti:...]`, `[ar:...]`) if they carry them. While the panel stays open, the next song's lyrics are looked up as it starts; closing the panel stops that.
- A row of buttons too long for its space has an arrow at the edge it carries on past; tap the arrow, or swipe the row, for the rest. On a phone, a view's buttons are Play, Shuffle and "…", which has the rest.

### Smart playlists

A smart playlist is a saved search that keeps itself up to date. Make one from Playlists with New smart playlist (`S` in the terminal app).

In the window it opens as rows, like iTunes: Match all or any of the rules, then a row per rule with a field, a condition and a value, and − and + to take a row out or add one under it. … adds a group under a row: a "Match any of" (or all) line with rules of its own, set in under it, so one playlist can say "4 stars and up, and any of these three genres". That's brackets in the text, and goes one level deep. Rating rows take stars, ranges take two numbers, Last played and the dates take "in the last 6 months" or a date. Under the rows, Limit to N songs, minutes, hours, MB or GB, and the order they're picked or sorted by (random, highest rating, most recently played, and so on). The count of matching songs updates as you change anything. Edit as text switches to the rules as text, for what rows can't say (a group inside a group, "not" in front of brackets, the leave-out box), and Edit as rules goes back when they fit. Drop-down lists that run longer than the screen scroll, and stay clear of a phone's system bars. The terminal app uses the text.

Every library starts with three by rating: One Star, Mid Tier (2 and 3 stars) and Best Rated (4 and 5 stars). Change or delete them like any other.

To take one to another device, right-click it (long press on a phone) for Export rules, or `}` in the terminal app. That writes its name and rules to a small `.smart.toml` file, which Import in Playlists (`<` in the terminal app) reads on the other device. The songs come from that device's library, so the same rules can match different songs there. A name that's taken gets a number after it, and rules the other device can't read are left out with a note saying which and why. The file is plain text, one `[[smart]]` table each with `name`, `rules` and `exclude`, so it's easy to edit by hand.

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
- On Android, Import opens the system's file picker and Export the system's save dialog, so a playlist comes from or goes to Downloads, a cloud drive or a USB stick, not the app's private folder.

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
- **Playing.** Songs stream as the original file, so bit-perfect and gapless work as with local files. Set a server's streaming to a conversion and Plex's own transcoder makes it: Opus at 96 to 256 kbps, the codec Plex's own players get, or MP3 320 for older gear. The cache downloads it and keeps it like any other. Plex sometimes fails at a download with HTTP 500, either way round: the original file or a conversion. The app asks once more, a second later. If Plex fails again, it asks for the song the other way: a conversion that won't come is downloaded as the original (bigger, but it plays), and an original that won't come is downloaded converted (Opus 256 kbps, or the cache's own conversion if one's set), so it plays rather than getting skipped. Either way it's kept under the name of what it is. The log says which songs, with Plex's own reason for a failed conversion when it gives one. A 500 doesn't count as the server being gone, since Plex answered; only a server that doesn't answer at all goes offline.
- **When a server is away.** Songs on a server that isn't connected aren't marked as broken. Three of them in a row stop playback with one message, rather than skipping through the rest of the queue; they play again once the server's back.
- **When a song won't play,** the note says why the download failed (the server's answer, or the connection dropping) rather than the decoder's "I/O error".
- **Ratings** go both ways (Plex counts out of ten, so a star is two).
- **Plays and Now Playing.** While a song plays, the app tells Plex where it is every ten seconds and whenever it pauses, resumes or stops, the way Plex's own players do. That puts PatinaPlayer on Plex's dashboard, and Plex counts the play once the song gets near its end, so "most played" and history in Plex stay right.
- **Playlists** come across, and changes to yours go back. One that won't load is skipped, the rest still come across, and the note says which and why.
- **Staying connected.** The app checks the server every 30 seconds. A missed check is asked again on a fresh connection before the server shows as offline, and a call that drops halfway is tried once more, so a phone moving between Wi-Fi and mobile data doesn't knock it offline. Plex's smart playlists come across too, read-only, and are read again every half hour since their songs change on their own.
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

- Stations from a snapserver (`snapcast://` addresses) show a SNAPCAST badge in Radio, so they're told apart from Icecast and the rest.
- Internet radio: any stream FFmpeg can decode, Icecast and SHOUTcast included. `.pls`, `.m3u` and `.m3u8` links resolve on their own, HLS goes straight to FFmpeg, and a dropped stream reconnects with backoff. Test a station and it tells you what the URL turned out to be.
- Podcasts: subscribe by feed URL, pick up where you left off, 0.5x to 3x speed with the pitch kept, silence trimming, and downloads for offline. While an episode plays, the speed chip on the bottom bar steps through 1x, 1.2x, 1.5x and 2x on a click, and right-click (or a long press) opens all the speeds and Skip silence. On a phone the chip sits on Now Playing beside the format; tap it for the same menu.

### Other devices

- **Play on** another device: the cast button on the bottom bar or on Now Playing, or the last entry in the output picker (Ctrl+T in the terminal app, which then shows where it's playing in the status bar). It lists Chromecasts, DLNA renderers (network speakers, AV receivers, TVs, Kodi) and Snapcast servers found on the network. The sound still comes from this app's player, after the EQ, the amp stack and leveling, so the queue, gapless and crossfades all carry on. Chromecasts and DLNA renderers fetch it from the app as a live 48 kHz WAV stream; a snapserver gets it over TCP. Pick This device to come back.
- For Snapcast, nothing has to change on the snapserver. When you pick it, PatinaPlayer asks the snapserver over its control API (port 1705, or 1780) for a TCP source on port 4953 and adds one called "Patina" if there isn't one, then checks it can connect, and says what's wrong if it can't. Then turn rooms on from the Snapcast page (below), or set them to the Patina stream in Snapweb or the Snapcast app. A source added that way lasts until the snapserver restarts, and PatinaPlayer adds it again the next time. To keep it for good, put `source = tcp://0.0.0.0:4953?name=Patina&mode=server` in the `[stream]` section of the `snapserver.conf` the snapserver actually reads (in Docker, the one mounted into the container), and publish port 4953 if it runs in Docker. A snapserver that doesn't announce itself goes in by address, from the same list or the Snapcast page.
- **Listening to Snapcast**: the Snapcast page also lists the snapserver's streams (all but Patina's own). Listen here plays one on this device and puts it in Radio as a station, `snapcast://host:1704/<stream>`, to come back to. PatinaPlayer connects as one more snapclient and plays what the server sends: FLAC, Ogg, PCM or Opus. It doesn't keep in step with the rooms the way a real snapclient does, so it runs about a buffer's length (a second, by default) ahead of them, and Snapweb's volume for it does nothing; the app's own volume does. It shows in Snapweb as "<your machine> (PatinaPlayer)" and never in PatinaPlayer's own room list, so it can't end up playing itself. Playing to a snapserver and listening to it at once would loop the sound, so Listen here stops playing to that one first. Icecast is the other way round from Snapcast: an Icecast server is something you listen to, so it's under Sources.
- **Snapcast rooms**: the Snapcast page lists the snapclients the snapserver knows. Turn one on and PatinaPlayer plays in that room, with nothing else changing: a room that shares a Snapcast group with others is moved into a group of its own first, so the others keep what they were playing. Turning it off puts it back on the stream it had. If the snapserver has no stream but Patina's, PatinaPlayer adds a silent one called "Patina-idle" (a TCP source on port 4954 that nothing sends to) and parks the room there, so its own volume and mute stay as they were. Like the Patina stream, it lasts until the snapserver restarts. Each room has its own volume there too. Turning the first room on starts playing on that snapserver if PatinaPlayer isn't already. It all goes through the snapserver's control API (`Group.SetClients`, `Group.SetStream`, `Client.SetVolume`).
- **Remote control**: turn it on in Settings, Other devices, and PatinaPlayer on a phone (or another computer) on the same network can steer this one: the song, its cover, play, pause, skip, seek, volume, shuffle, repeat, the heart, and the queue, where tapping a song plays it. It asks for the six-digit code shown there; New code makes a fresh one and shuts out whoever had the old one, without restarting the server. On the other side, Settings, Other devices, Control another player lists the players on the network (or takes an address) and opens their Now Playing. Back or Escape leaves it. The code from last time is kept, so the second time it's one tap.

### The desktop

- MPRIS2, so media keys, Plasma's and GNOME's media controls and `playerctl` all work, with the cover.
- A tray icon (StatusNotifierItem). Closing the window hides it there and the music keeps going, unless you'd rather close meant quit.
- The window steps down from a wide desktop to phone size without cutting anything off. A row of buttons that doesn't fit scrolls sideways. The sidebar shows names when there's room and folds to icons when there isn't. The button at the bottom of the sidebar, or Settings, Appearance, sets it to stay expanded or folded, separately for landscape and portrait.
- Sidebar groups fold: click Sources or Play on to hide or show what's under it. Once there's a library folder or a server set up, both start folded, since they're set-up pages you don't visit daily. What you fold or open is kept (`sidebar_folded` in the config).
- Settings has a search: with Settings open, the search box finds settings by words in their name, their help line or their group (`gapless`, `podcast speed`, `cache size`). Groups with a match open on their own. Clear the box to see them all again.
- Themes: Runout (dark verdigris and copper, the default), Light, Dark, OLED (true black), and Kawaii Light and Kawaii Dark (pastel, rounded, with Japanese section names). The terminal app follows the theme too.
- Fonts are bundled, so it looks the same everywhere.
- Wayland or X11: Settings, Desktop, Display server. Automatic uses Wayland when the desktop has it; X11 goes through XWayland on a Wayland desktop. It takes effect the next time the app starts. Starting it with `WAYLAND_DISPLAY=` in front does the same once.
- The visualizer moves at about 60 frames a second whatever the output buffer is: it works out which samples are being heard at each frame rather than waiting for the next buffer to come round, so a big `buffer_ms` doesn't make it jumpy.

### The terminal app

- Every view, the EQ, the amp stack, Settings and the rest, from the keyboard, with mouse support.
- `:` opens a command palette: type a few letters of what you want (`eq`, `sleep`, `cast`, `undo`) and Enter runs it. Each line shows its key, so it doubles as a way to learn them.
- `/` in Settings finds a setting by words, as in the window.
- Cover art with kitty graphics, sixel or iTerm2 images where the terminal supports them, and half blocks everywhere else.
- Keeps playing when you detach tmux or screen.

### On Android

- The same window and the same core, for arm64 phones on Android 9 or later.
- Your Music folder is in the library from the first start. Add more with Android's folder picker.
- Tap opens and plays, long press opens the menu that right-click opens on the desktop. Text is typed in Android's own dialogs, which stay clear of the keyboard.
- A mouse works too (a USB or Bluetooth one, or a tablet's touchpad): the wheel scrolls, right-click opens the menu, and a drag with the button held drags. Slint's Android backend crashed on the wheel, so the APK build patches it; see [The APK, locally](#the-apk-locally).
- Music keeps playing with the screen off, with the notification, lock screen controls, Bluetooth buttons, and pausing when headphones come out or a call comes in.
- Android Auto works with the app closed on the phone: connecting the car starts the player in the background, with your last queue where you left it, and opening the app later carries on with the same player. The car shows what's playing with next, previous and play/pause, plus a heart for the song playing and a shuffle button. Its tabs are Home (Shuffle everything, Favorites, Recently played, Recently added, Mixes), Artists, Albums, Playlists (smart playlists included) and Folders, with album covers in the lists and as grids. A long list of artists or albums comes as one folder per letter. A car with room for only four tabs gets Home at the top of Playlists. "Play Nirvana on PatinaPlayer" plays an artist, album or songs that match.
- A home screen widget with the cover, the song, and back, play or pause, and next, in the app's theme. It keeps the theme after a restart.
- The System theme (Android 12 and later) follows your wallpaper's Material You colors and the phone's light or dark mode.
- The volume is the phone's media volume: the volume buttons and the app's slider move the same thing, and the slider follows the buttons. The first time, whatever the app's own volume was goes into the phone's, so nothing gets louder. On Now Playing, the volume button opens a tall slider, easier to hit with a thumb than the bar's. The bar at the bottom leaves the volume and the sleep timer to Now Playing on a phone, so the song's title has room.
- A light buzz under the finger on a long press, on picking rows, and on each letter of the scrubber, following Android's own touch feedback setting.
- When the sound moves to another device on its own (a Bluetooth headset connects or goes away), the song pauses where it was. Play carries on from there, on whichever device has the sound now.
- The Drive screen (Minimal UX while driving, in Settings, Appearance, on by default): when the phone goes into car mode, which Android Auto turns on and so do driving modes built on it, the app shows a car-style screen instead of the usual one. The cover, the song in big type, a thick progress bar, huge previous, play and next buttons, and the heart, shuffle and volume under them. Swipe anywhere left or right for the next or previous song. On a landscape screen the cover sits beside the buttons. Full app goes back to the whole app until driving ends, and a Drive button in the corner brings the Drive screen back. When the phone leaves car mode it all goes back to normal.
- Settings leaves out what doesn't apply on a phone: the Desktop group (tray icon, close to tray), the iPod group, the Files group (paths inside the app's private folder), Sample format and Dither (AAudio always takes float) and Hold on metered connections (that comes from NetworkManager, which Android doesn't have).
- Back up the library (Settings, Library) saves a copy of the library database through Android's save dialog, to Downloads, Drive or wherever you pick. It's the database the app keeps in its private folder: play counts, ratings, playlists, smart playlists, listening history and podcast positions. Passwords and tokens aren't in it; they're in the keyring.
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

### FreeBSD

Download `PatinaPlayer-<version>-freebsd-amd64.tar.gz` from Releases.

1. Install what it needs: `pkg install pipewire alsa-lib gnutls fontconfig freetype2 libxkbcommon wayland libX11 libXcursor libXrandr libXi mesa-libs`
2. Unpack it: `tar -xzf PatinaPlayer-*-freebsd-amd64.tar.gz`
3. Go in and copy it into place: `cp -R bin share /usr/local/` as root, or `cp -R bin share ~/.local/` for just you, with `~/.local/bin` on your PATH.
4. Run `patinaplayer`, or `patinaplayer-tui` for the terminal app.

FFmpeg is built in, as in the AppImage. Sound goes through PipeWire when it's running, and through alsa-lib otherwise, which plays to OSS on FreeBSD. The tray icon needs a desktop with StatusNotifierItem (Plasma, or a panel plugin for it). This build is made in a FreeBSD VM on every release; I don't run FreeBSD myself, so tell me if something's off.

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

### FreeBSD, locally

`bash packaging/build-freebsd.sh`, on FreeBSD. It needs `pkg install rust git gmake pkgconf nasm gnutls llvm pipewire alsa-lib libxkbcommon wayland fontconfig freetype2 libX11 libXcursor libXrandr libXi mesa-libs curl bash` (or rustup for Rust). It builds the static FFmpeg with gmake, then both apps, and packs `target/freebsd/PatinaPlayer-<version>-freebsd-<arch>.tar.gz`.

FreeBSD's libutil has a `pw_init()` of its own, so FreeBSD's PipeWire renames PipeWire's `pw_init` and `pw_deinit` to `pipewire_init` and `pipewire_deinit`, with `pw_init` left as a macro. The `pipewire-sys` bindings come out without `pw_init`, and the `pipewire` crate stops with "cannot find function `pw_init` in crate `pw_sys`". `packaging/patch-pipewire-freebsd.sh` fixes that for the build, the same way the Slint patch works for the APK: it fetches `pipewire-sys` at the version `Cargo.lock` has, checks it against the lockfile, applies `packaging/pipewire-sys-freebsd.patch` (bindgen picks up the real names, and on FreeBSD only they're given their usual ones), and hands it to cargo with `--config` for that one build. Linux builds never see it. Once `pipewire-sys` takes a fix of its own, the script notices and builds without it.

A plain `cargo build` on FreeBSD needs the same `--config` line; the script prints the folder to point it at: `cargo build --release --config "patch.crates-io.pipewire-sys.path=\"$(bash packaging/patch-pipewire-freebsd.sh target/pipewire-freebsd)\""`

### The APK, locally

`bash packaging/build-apk.sh`. It needs the Android SDK with build-tools and a platform of 34 or newer (`ANDROID_HOME`), the NDK (`ANDROID_NDK_ROOT`), JDK 17 or newer, cmake, `curl` and `patch`, and `rustup target add aarch64-linux-android`. It builds FFmpeg and Mbed TLS for Android first, then the Rust library, the Java side and the APK, without Gradle. The result lands in `target/apk/`.

Slint's Android backend panics on a mouse wheel (scroll events hit a `todo!()` in it, as of Slint 1.19) and took a mouse click as a finger press on top of the button press. `packaging/patch-slint-android.sh` fixes that for the build: it downloads the backend at the version `Cargo.lock` has, checks it against the lockfile's checksum, applies `packaging/slint-android-mouse.patch`, and hands the patched copy to cargo with `--config` for that one build. Nothing is pinned and nothing is vendored in the repo, so it follows Slint as `cargo update` moves it. `Cargo.lock` is put back the way it was afterwards. If a Slint release changes that code so the patch no longer applies, the build stops and says so while the wheel still crashes, or warns and goes on without the patch once Slint handles it itself.

Without a release key in the environment it signs with a debug key kept in `target/android-debug.keystore`, which is fine for your own phone. The script's header lists the four signing variables.

## Making a release

The Release workflow builds the AppImage, the APK and the FreeBSD tarball from a tag and attaches them to that tag's release. It only runs when you start it, never on a push.

1. Go to Releases on GitHub and click Draft a new release.
2. Under Choose a tag, type a new tag (like `v1.2.0`) and create it there, on `main`.
3. Write the notes and click Publish release.
4. Go to Actions and click the Release workflow.
5. Click Run workflow, pick that tag under "Use workflow from", and run it.
6. Wait. The first run takes about 15 minutes; later ones are faster, since FFmpeg and the Rust build are cached. The FreeBSD build runs in a VM and isn't cached, so it can take longer than the others. The AppImage, its `.zsync`, the APK and the FreeBSD tarball land on the release you made. If one of them fails, the others still go up.

If it was run on a branch, or there's no release for the tag yet, it stops in the first few seconds and says which. Running it again on the same tag replaces the files on the release.

To try the builds before making a release, run the Test build workflow instead: Actions, Test build, Run workflow, on whichever branch. It runs the same three builds with no release needed and publishes nothing; the files are on the run's page under Artifacts. Once it's green, make the release and its tag as above.

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

Everything is in `~/.config/patinaplayer/config.toml`, and every option can be changed from Settings in either app. Settings is folded into groups (Library, Appearance, Playback and the rest), all closed when it opens; tap or press Enter on one to open it, and Open all or Fold all at the top does the lot. An open group's settings sit indented under its name, on a panel in the idle-selection color, so you can tell where one group ends and the next begins. Settings writes straight back to the file and keeps your comments. A typo in a key name is an error, not a silent default, so you find out right away.

```toml
[library]
folders = ["~/Music", "/mnt/storage/music"]
watch = true             # pick up changes while running
scope = "all"            # what browsing shows: "all", "local" or "server:Name"
source_order = "local, plex"  # with all shown, who an album shared by sources comes from

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
# sidebar_folded = ["Sources", "Play on"]   # sidebar groups kept folded; set by clicking them

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
center = true            # synced lyrics keep the playing line in the middle

[desktop]
tray = true
close_to_tray = true     # closing the window hides it there and keeps playing
display = "auto"         # auto (Wayland when there is one) or x11; read at start

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
| Library index, playlists, smart playlists, play counts, listening history, ratings, podcast positions, loudness, waiting scrobbles and listens | `~/.local/share/patinaplayer/library.db` (Settings, Library, Back up the library saves a copy; on the desktop it goes to Downloads, under a dated name) |
| Songs kept offline, downloaded episodes | `~/.local/share/patinaplayer/offline/` |
| Cached and prefetched songs | `~/.cache/patinaplayer/tracks/` |
| Cover art at the sizes the app draws | `~/.cache/patinaplayer/art/` |
| Artist pictures, and which artists have none | `~/.cache/patinaplayer/artists/` |
| Server passwords, the Last.fm session, the ListenBrainz token | your keyring, or `~/.config/patinaplayer/secrets.toml` (only you can read it) when there's no keyring |

To restore a backup on the desktop, quit the app and copy it over `library.db`. On Android the app's folder is private to it, so a backup can't be put back from outside the app; there's no restore button yet.

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
| < / > | Import a playlist file or a smart playlist file / export the selection or view as M3U8 |
| } | Export the selected or open smart playlist's rules, to import elsewhere |
| Ctrl+T | Play on another device: a Chromecast, a DLNA renderer or a Snapcast server |
| Ctrl+Z | Undo: put back what was just deleted or taken out (for 10 seconds) |
| : | Command palette: find any command by name and run it |
| o / O | Sort Albums or Songs / reverse Songs |
| B | Browse the next source: All sources, Local, each server |
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

In Radio: `N` new station, `E` edit, `T` test, `D` delete, `J`/`K` move. In Podcasts: `N` subscribe, `D` unsubscribe; in a show, `w` downloads, `x` deletes the download, `P` marks played. In the Queue: `d` remove (the picked songs, if any), `J`/`K` move, `S` save to a playlist (the picked songs, if any), `L` load the queue saved on your server, `C` clear. `D` and `C` don't ask; Ctrl+Z puts it back. In Playlists: `N` new, `S` new smart playlist, `R` rename, `D` delete, `E` edit a smart playlist's rules. In Recently played: the Clear the history row at the top empties it. In the iPod view: `d` take a playlist or album off, `E` eject. In Downloads: `w` pauses or resumes, `x` cancels or unpins, `W` pauses everything, `C` clears the cache. In Settings and the EQ: Enter flips, cycles or edits, `h`/`l` step a value down or up. `/` in Settings finds a setting. On the Sources pages the same, plus `E` renames or edits a server and `d` removes it; Sign in shows the Plex code in the message line, and adds your own server once it's entered (sign in again for another on the same account).

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
packaging/          AppImage and APK builds, FFmpeg for each, and the patch for Slint's Android backend
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
- The source picker hides albums by artist and album name. Two copies named differently (a deluxe edition on one server, the plain one locally) both show.
- A song streaming as a conversion (Opus or MP3) has no known size until it's done, so a seek past what's downloaded waits for the download to get there. The original file seeks straight away.
- One download runs at a time, so the most urgent one gets the whole line. A big pinned library on a slow line takes a while.
- DLNA sources are songs only: DLNA has no playlists, queues, ratings or stars to reach, and no way to tell the server about plays. Finding DLNA servers needs multicast, so one on another subnet or behind a VPN has to go in the config by its description address.
- Navidrome makes its folder tree up from tags (Artist/Album/01 - Title.flac) unless its `Subsonic.DefaultReportRealPath` option is on. Navidrome has no podcasts, and Ampache has no saved play queue and no lyrics call.
- Mixes match Last.fm's answers to your library by artist and title as tagged, apart from case. "The Beatles" and "Beatles" don't match.
- A tag edit on the server that leaves an album's song count, length and dates alone isn't seen by the regular sync. Sync now, on its Sources page, reads every album and catches it.
- Listening to a Snapcast stream plays it as it arrives, not at the time stamped on it, so it runs about a second ahead of the rooms, and that client's volume in Snapweb does nothing. Patina's listener is tested against a stand-in snapserver, not yet a real one.
- The Info page, finding cover art and artist pictures are the window's (desktop and Android). The terminal app has none of them.
- Plex is built from Plex's published API and tested against a stand-in that follows it, not yet against a real Plex Media Server. Some developers report that a server token plex.tv hands out for a Plex JWT gets refused by their server; the app then tries the account's token, and if that's refused too, Add by hand with a classic token works. Plex has no favorites, so stars don't sync with it, and no saved play queue. Plex's podcasts and its Live TV aren't music libraries and don't come across.
- A Plex conversion comes as HLS segments (the only way Plex's docs offer it), joined into one file as it downloads, so like any conversion it seeks only as far as what's downloaded.
- Icecast needs 2.4 or newer for its JSON status page. A stream that needs a password to listen won't play.
- Metered connections are only detected through NetworkManager.
- inotify needs one watch per folder, so a very large library can run past `fs.inotify.max_user_watches`. The app says so if it does, and raising the limit with `sysctl` fixes it.
- On Android, a library backup can be saved but not restored from the app yet.
- On Android, folders are added by path, so a folder from a cloud app's storage provider can't go in. Add that music through a server instead. Bit-perfect depends on the phone's maker offering it.
- Covers are found by the album's tags, so an album tagged wrong turns up wrong covers or none. Fix the tags, or use an image file. Android Auto has been tried in one car so far, and its queue screen not yet in a car. The heart and shuffle icons come from Android Auto itself on newer versions; an older version draws the app's own.
- The home screen widget's buttons control the player while the app is running. After the phone restarts, or Android stops the app, they open it instead, since there's no player to tell until it starts. With the System theme, the widget picks up a new wallpaper or a switch between light and dark mode the next time the app runs, since only the app can work out the colors.
- The iPod sync covers what libgpod covers: iPod classic, the older nanos (up to the 5th generation), minis, and shuffles up to the 2nd generation. The 6th and 7th generation nanos and the iPod touch don't work. It's desktop only. Play counts and ratings made on the iPod don't come back.
- In the terminal app, smart playlists are typed rules, not a form of drop-downs. The dialog checks them as you type and the examples are one click away, but it's still a small language to learn. The window's rows go one group deep; anything deeper is text there too.
- FFmpeg doesn't check certificates on HTTPS radio and podcast streams. Server, Last.fm and lyrics calls go through the app's own HTTP client, which does.
- Playing on a Chromecast or a DLNA renderer runs a few seconds behind: they buffer before they play, so pause, skip and the volume take that long to be heard, and the position shown runs ahead of what you hear. Snapcast is as late as its own buffer, a second by default. The app's player keeps time, so a song that ends here moves on there too. A Chromecast shows "PatinaPlayer" rather than the song. Some DLNA renderers don't play a live WAV stream at all.
- The remote control is plain HTTP on your network. The code keeps out the curious, not someone watching the network, and it sits in `config.toml` as written. Finding players and devices needs multicast (mDNS, SSDP), like finding DLNA servers.
- Crossfeed is a simple bs2b-style crossfeed, not a measured head model. Width, balance and crossfeed only touch stereo; anything with more channels passes through untouched.

## License

GPL-3.0-or-later. See `LICENSE`.

The bundled fonts (Inter, Noto Serif, JetBrains Mono, M PLUS Rounded 1c, Zen Maru Gothic) are under the SIL Open Font License, in `crates/patina-gui/ui/fonts/`.
