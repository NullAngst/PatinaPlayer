# PatinaPlayer

PatinaPlayer is a Linux music player for Subsonic, Ampache and DLNA servers and local folders, for people who want a minimal interface on top of a serious playback chain.

This is milestone 11 of the build plan: everything from milestones 1 to 10 (the audio engine, the library, radio, podcasts, both apps, cover art, listening stats, the AppImage, bit-perfect output, an editable Settings view, playlists from any view, the themes, Subsonic, Ampache and DLNA servers, the song cache with prefetch, offline pins and offline mode, mixes, volume leveling, and the EQ and true-peak limiter), plus Last.fm: linking your account from Settings, Now Playing, scrobbling with an offline queue, love, and a choice per server of who scrobbles its songs, so nothing counts twice. There are two programs on one core. `patinaplayer` is the desktop app and `patinaplayer-tui` runs in a terminal. Both show the same views, play through the same engine, and read and write the same library, playlists and config. Output controls (device picker, sample rate modes, resampler, dither, crossfade) come next, then the rest. What's here is built to the final shape, so later milestones add to it instead of replacing it.

## What works right now

**Music servers**
- Subsonic servers (Navidrome, Gonic, Airsonic, LMS and anything else that speaks the Subsonic API) and Ampache, as many as you like, each one a source in the sidebar next to your folders.
- Add one in Settings, under Servers: a name, its address, your user name and password. The app logs in to find out what kind of server it is, so there's nothing to pick. Ampache gets its own API, since Ampache speaks Subsonic too but its own API carries more.
- The server's whole library is copied into the local index, so its songs turn up in Artists, Albums, Songs, Genres, search, the listening stats and Folders, mixed in with your local music, and browsing never waits on the network. Server albums say which server they're on, so they can be told from a local copy.
- The first sync reads everything. Later ones (every half hour, and at startup) only fetch albums that changed. Sync now in Settings reads it all again.
- Songs stream as the original file by default, so the sound chain gets the real thing and bit-perfect works for them as it does for local files. Settings can switch a server to Opus or MP3 at 96 to 320 kbps for a slow line.
- Cover art comes from the server and is cached on disk at the sizes the app draws.
- Server playlists show in Playlists with the server's name. Add to them, take songs out, reorder, rename or delete, and the change goes to the server. Someone else's playlist (or an Ampache smart list) is read-only. Add to playlist offers New playlist on <server> too.
- The play queue is saved on the server a few seconds after it changes and every half minute while playing, so another client can pick up where you left off. Queue from <server> in the Queue view (`L` in the TUI) loads the one saved there. Only Subsonic servers keep a queue; Ampache's API has none.
- The server's radio stations show under your own in Radio, and its podcasts (the episodes the server has downloaded) show in Podcasts, with your place in them saved here and sent to the server as a bookmark.
- Passwords and API keys go in the desktop keyring through the Secret Service API (KWallet on Plasma, GNOME Keyring elsewhere), never in config.toml. With no keyring running (a TTY, the TUI over SSH), they go in `~/.config/patinaplayer/secrets.toml`, readable only by you, and the app says so when it does it.
- Logins use a salted token, or an API key where the server offers OpenSubsonic API keys (leave the user name empty and put the key in the password box). The plain password is never put in a URL.

**Mixes**
- Genre mix: mark one or more genres (`M` in Genres, or right-click, Mark for a mix) and play the mix (`X`, or the Play mix button). It takes every song on the albums tagged with all of the genres you marked, shuffled up front. Blues Rock and Post-Grunge together gets you albums carrying both tags, not every album with either. Settings, Genre mixes match, switches from album tags to each song's own tags.
- Artist mix: the artist and similar artists, shuffled, a quarter of it the artist themselves. Start one with `X` on an artist or an album, right-click, Artist mix, or the Mix button on an artist or album page.
- Track mix: songs similar to one song, with that song first. `X` on a song, or right-click, Track mix.
- Artist and track mixes are endless. When fewer than ten songs are left in the queue, the next 25 go on the end, so prefetch always has something to fetch. A song picked in the last 100 isn't picked again, and the same artist doesn't come up twice in a row when there's anyone else.
- Who's similar comes from your server first (Navidrome, Gonic, Airsonic and Ampache get it from Last.fm, if they're set up for it). When the server has nothing, PatinaPlayer asks Last.fm itself, if you've given it an API key. When neither knows, it goes by your own tags: artists and songs that share genres with what you picked. That last way works with local folders and no key at all.
- Only what's in your libraries plays. A similar artist you don't have is skipped. Every source counts, and a song that lives in two places plays once, from a local copy if there is one.
- While a server is unreachable, a mix leaves out its songs that aren't kept.
- Save a mix you like with Save mix on the Queue page (`M` in the Queue). It goes in the sidebar under Mixes, one click to play, and in Playlists under Saved mixes, where you can rename it, delete it, or keep it offline. A saved mix remembers who was similar when you saved it, so it plays the same way without the server or Last.fm.
- Playing anything else ends the mix. Adding to the queue doesn't.

**Volume leveling**
- Evens out loudness between songs, the ReplayGain 2 way: every song brought to a target loudness, -18 LUFS by default. On by default, in Smart mode.
- Modes, in Settings under Leveling: Off, Track (every song on its own), Album (an album shares one gain, so a quiet intro stays quiet), and Smart. Smart uses the album gain while an album plays in order and the track gain on shuffle, in a mix, or in a playlist that jumps between albums. "In order" means a neighbour in the queue is the same album with the next track number.
- Where a song's gain comes from, best first: the server's replayGain field (Navidrome sends it), ReplayGain tags in the file, then PatinaPlayer's own measurement.
- Songs with neither get measured with EBU R128 (integrated loudness and true peak, the figures ReplayGain 2 is made from) and the result goes in the library index, so each song is measured once. What gets measured: the song playing and the next five in the queue, server songs as soon as they land in the cache (prefetched, played or pinned), and, if you press Measure now under Leveling, every untagged song in your folders, in the background.
- A server song is measured from its cached file, so nothing is downloaded twice. That file is checked for ReplayGain tags first, since some servers (Gonic, for one) don't pass them on.
- An album with no album gain in its tags gets one worked out from its songs once every song on it is known.
- A song nothing is known about yet plays at -6 dB (Gain before measuring, in Settings), and plays at its measured gain from the next time on. Since prefetch downloads and measures ahead, that's mostly the first song you play.
- Keep peaks from clipping (on by default) holds a song's gain down so its loudest peak stays under full scale. It matters when you raise the target. The limiter catches whatever's left.
- The live normalizer follows the sound as it plays: the last 3 seconds of loudness, moving the gain about a dB a second (faster for the first few seconds), holding still through silence. It's on for radio and podcasts, which can't be measured ahead of time, and off for music, since a measured gain sounds better. Turn it on for music and it takes over for songs that aren't measured yet, instead of the fixed -6 dB.
- The gain switches at the exact sample where one song becomes the next, so gapless albums stay gapless. A test plays a FLAC cut into three parts with leveling on and checks the output is the original, scaled, sample for sample.
- The bottom bar shows the gain on what's playing: `-4.2 dB`, `-6.0 dB?` while the song isn't measured yet, `~-9.1 dB` under the live normalizer. Click it in the desktop app for where it came from (the server, tags, or measured here).
- Changes in Settings apply to the song playing too, within a couple of seconds, gliding to the new gain instead of jumping.
- Bit-perfect skips leveling. With bit-perfect On, the leveling rows grey out and say why. Auto turns bit-perfect on only when leveling is Off, since leveling changes the sound.

**Last.fm**
- Link your account in Settings, under Last.fm: Link opens Last.fm's page in your browser, you approve PatinaPlayer there, and the app finishes on its own (it checks every few seconds for five minutes). The session key goes in your keyring, never config.toml. Unlink forgets it.
- Now Playing goes out as a song starts. A song is scrobbled once you've listened to half of it or four minutes, whichever is first, and only if it's longer than 30 seconds: Last.fm's own rules. Seeking ahead doesn't count as listening.
- Every scrobble goes into the library database first and leaves once Last.fm takes it. Offline, or with Last.fm down, they wait, and go out later in batches of 50, each with the time it was played. Settings shows how many are waiting.
- If Last.fm stops accepting the session (you revoked the app on the site, say), it says so and the scrobbles wait for you to link again.
- Love the song playing with `l` or the heart in the bottom bar. It's kept in the library and sent to Last.fm, and when a song starts, Last.fm is asked whether you'd already loved it. Love also stars it on the server, if you turn that on.
- Who scrobbles a server's songs is set per server (Settings, Servers, `name: scrobbles`): this app (the default), the server, or both. Navidrome and others pass plays on to Last.fm themselves when the player reports them, so if yours does, pick the server, or every play counts twice. Picking both asks first, with that warning. When Last.fm isn't linked here, the server is always told, since nothing can double up, so the server's play counts keep going up.
- Podcasts never scrobble. Radio can, from the station's song titles ("Artist - Title"), once a title has been on a minute; that's off by default, since station metadata is often junk.
- A Last.fm API key and secret can be built in: the AppImage workflow takes them from the repository's `LASTFM_KEY` and `LASTFM_SECRET` secrets. Your own (free from last.fm/api/account/create) in Settings win over them. A source build without either needs your own to scrobble.

**EQ**
- An EQ view in the sidebar (`Ctrl+E` from anywhere, or click EQ in the bottom bar), with the response curve on top and every setting under it.
- Graphic mode: 10 bands an octave apart (31 Hz to 16 kHz), or 15 (two-thirds octaves) or 31 (third octaves), ±12 dB each. Switching between them keeps the curve. In the desktop app, drag a band up or down on the curve; in the TUI, `h` and `l` move the selected band half a dB, or Enter and type a value.
- Parametric mode: up to 20 bands, each a peak, low shelf, high shelf, low pass, high pass or notch, with its own frequency, gain and Q. Drag a band's dot anywhere on the curve in the desktop app. In the TUI, type a band the way AutoEq writes them (`PK 105 Hz 6.7 dB Q 0.7`, or just `LS 100 4`), `h`/`l` move its gain, `H`/`L` its frequency, `d` removes it.
- The curve is drawn from the same filter coefficients that play, so where neighbouring bands overlap and add up, you see that too.
- Preamp, plus an auto preamp (on by default) that takes the biggest boost off, so the EQ can't push the sound into the limiter. The row says how much it's taking off right now.
- On and bypassed is one switch (`o` in the TUI, the On chip on the curve) for quick A/B. It glides over 20 ms, so it doesn't click.
- Presets: Flat and a dozen built-in genre curves as starting points, and your own. Save as keeps what you have under a name, as a TOML file in `~/.config/patinaplayer/presets/eq/`, easy to copy or share. A preset you've changed shows as "Rock (changed)".
- Import takes an AutoEq ParametricEQ.txt or GraphicEQ.txt, or an Equalizer APO config. Parametric filters come in exactly, with AutoEq's preamp. A GraphicEQ curve (hundreds of points) is fitted to the 31-band graphic EQ. The import becomes a preset named after the file ("Sennheiser HD 650 ParametricEQ.txt" is "Sennheiser HD 650"). Whatever APO does that this EQ can't (per-channel filters, includes) is skipped, and a note says so.
- Per-device EQ: on the Device row, Keep this EQ for it gives the device playing now an EQ of its own, saved as a preset named after it. From then on, whenever the sound goes to that device, its EQ comes back; every other device gets your usual EQ. Changes you make while it plays are saved to its preset. On PipeWire this follows the default output, so switching from headphones to speakers in Plasma's volume applet switches the EQ with it, mid-song. With a device set in Settings, or on ALSA, it goes by that device.
- Changes are heard within one output block (a few milliseconds) and saved half a second after you stop, to `[eq]` in config.toml.
- The bottom bar's EQ chip is lit while the EQ is on and changing something.

**Limiter**
- A true-peak limiter last in the chain, after the EQ and the volume, so EQ boosts and leveling never clip. On by default, with a -1 dBTP ceiling (Settings, Limiter).
- It catches peaks between samples too, the ones a DAC's reconstruction filter makes, by looking at the signal 4x oversampled.
- It looks 2 ms ahead in the buffer, so it's already down when a peak arrives, and lets go over about a tenth of a second. Looking ahead adds no delay: it reads further into audio that's already decoded.
- Below the ceiling it doesn't touch a single sample. The gapless test plays through it and still matches the source bit for bit.
- While it's working, the bottom bar shows how much it's taking off (`LIM -2.2 dB`, in copper), held for a moment so you can see it.
- Bit-perfect playback skips it, along with everything else.

**Cache and offline**
- Server songs are kept on disk in three ways. Prefetch downloads the songs coming up in the queue, nearest first. The cache keeps what you've played, so a replay doesn't hit the server, and drops the least recently played songs once it's full. Pinned songs stay until you unpin them.
- Every server song plays through the cache. One that's on disk opens as a local file. One that isn't plays from its download while the download runs, so it starts as soon as the first bytes arrive, then it's kept for next time.
- Seeking in a song that's still downloading doesn't wait for the download to get there: the player asks the server for that spot directly with a range request, then goes back to the downloaded file once the download catches up.
- Prefetch goes by size (1 GB by default) or by number of songs, set in Settings under Cache and offline. Size is the default, since twenty hi-res FLACs and twenty MP3s are very different amounts of disk. It comes out of the cache size (10 GB by default), and Settings won't let you set more prefetch than cache.
- Keep offline (`f`, or right-click) pins a song, an album, an artist, a playlist or a server folder. `F`, or the Keep offline button at the top of the view, pins the whole thing you're looking at. A pinned playlist or artist that grows gets its new songs after the next sync.
- Unpinning doesn't delete anything outright. The songs move back into the cache and leave once it needs the room.
- A server that stops answering goes offline within half a minute, or straight away if a download from it fails. Its songs that aren't kept grey out instead of vanishing, and Play leaves them out. Pinned and cached songs play as before. When the server answers again, everything comes back.
- Downloads shows how full the cache is, what's pinned, and the download queue. In the queue, `w` pauses or resumes a download and `x` cancels it, `W` pauses everything but the song playing, and `C` clears the cache (pinned songs stay).
- Background downloads (prefetch and pinned songs) hold while NetworkManager says the connection is metered, like a phone hotspot. Turn that off in Settings if you'd rather they didn't.
- Songs are kept as the original file by default. Settings, Keep songs as, switches to Opus or MP3 that the server converts, to save space. Converted songs can't play bit-perfect. A server set to stream a conversion keeps doing that for what it downloads too.
- Downloads resume where they stopped, with a range request, after a dropped connection or a restart.
- Podcast episodes download through the same queue and show up in it.
- The cache lives in `~/.cache/patinaplayer/tracks` and pinned songs in `~/.local/share/patinaplayer/offline/tracks`, under readable names (`Home/Nirvana/Bleach/01 Blew.flac`). They're split because cleaners like BleachBit wipe `~/.cache`, and nobody should lose a pinned library to that. If something does wipe the cache, PatinaPlayer notices at startup and forgets what went.

**DLNA servers**
- MiniDLNA, Gerbera, Jellyfin, Plex, Serviio, Twonky and the media server on most NAS boxes, as sources in the sidebar like any other server.
- No address to type and no login. Settings, Find DLNA servers asks the network who's there, lists what answered (name, model, address), and you pick one. DLNA has no passwords, so nothing goes in the keyring.
- Its library is copied into the index like a Subsonic server's, so its songs are in every view, search and the stats, and browsing never waits on the network.
- The library is read the best way the server allows. A folder view (MiniDLNA's Browse Folders, Gerbera's PC Directory, Serviio's and Twonky's Folders) gives the real folder tree, and where the server can also search, the tags come from the search, since some folder views title songs by file name. Without a folder view it searches for every song. Without either it walks the whole tree, and keeps each song once even though it shows under Artist, Album, Genre and All Tracks.
- A sync only walks the server again when the server says its content changed (its update id), or every six hours, since some servers never say.
- Songs stream as the original file, so bit-perfect works for them. DLNA servers that offer a converted copy alongside the original get the original.
- Cover art comes from the server, or from a cover.jpg in the song's folder where the server lists one.
- Songs are kept without the server's address, and the server is remembered by its unique id. If its address changes (a router handing out a new IP), PatinaPlayer finds it again on the network and the songs, play counts and playlists stay as they were.
- Edit on a DLNA server renames it. There's nothing else to change.

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
- Six looks, picked in Settings under Appearance. The switch is instant, no restart.
- Runout: the default dark verdigris and copper.
- Light: white and cool grey with one blue accent, and Inter for the headings. A plain light theme with nothing to get used to.
- Dark: the same on neutral charcoal.
- OLED: true black for the background, the sidebar, the bars and the dialogs, so an OLED screen switches those pixels off. Panes are told apart by thin borders instead of shades of grey. It's for OLED monitors now, and it's what phones will want when the Android app comes.
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
- Auto mode means "bit-perfect whenever nothing would change the sound": leveling Off, and the EQ bypassed or flat. With leveling on (the default), that's never, so Auto is for people who set leveling to Off.
- With bit-perfect On, leveling, the EQ and the limiter are skipped, and the leveling and EQ settings grey out with the reason.
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
- Reads tags with lofty (ID3v2, Vorbis comments, MP4, APE), with FFmpeg as the fallback for anything lofty can't read. ReplayGain values go in the index, where leveling finds them.
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
- Downloads: how full the cache is, what's downloading, what's pinned, and downloaded podcast episodes.
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

To add a music server:

1. Open Settings and find Servers.
2. Click Add server… (in the TUI: select Add a music server and press Enter).
3. Give it a name for the sidebar, its address (like `https://music.example.com`, or whichever address you open its web page at), your user name and your password.
4. Connect. If the login works, it's saved, and the sync starts. Progress shows next to its name in the sidebar.

Now the server's albums fill in under Albums alongside your folders, and its playlists, stations and podcasts show up in their views.

To add a DLNA server:

1. Make sure it's on and on the same network as you (same router, same subnet).
2. Open Settings and find Servers.
3. Click Find servers… next to Find DLNA servers (in the TUI: select it and press Enter). It listens for about three seconds.
4. Pick the server from the list. It's added under its own name and the sync starts.

If nothing answers, the note says so. A firewall that drops incoming UDP is the usual cause: the answers come back to a random port, so ufw (or whichever firewall you run) has to let replies from the server's address in. Kubuntu ships with ufw off.

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
theme = "runout"         # runout, light, dark, oled, kawaii-light or kawaii-dark

[playback]
volume = 80              # starting volume, 0 to 100
fade_ms = 100            # 0 turns fades off

[leveling]
mode = "smart"           # off, track, album or smart
target_lufs = -18        # -23 to -10; higher is louder
fallback_db = -6         # -12 to 0, for songs not measured yet
clip_guard = true        # keep the gain under the song's peak
live_music = false       # the live normalizer for unmeasured music
live_radio = true        # the live normalizer for radio and podcasts

[limiter]
on = true                # the true-peak limiter, last in the chain
ceiling_db = -1.0        # -6 to 0 dBTP

[eq]                     # the app writes this table; the EQ view is easier
on = true
mode = "parametric"      # graphic or parametric
graphic_bands = 10       # 10, 15 or 31
graphic = [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]
preamp = -6.4
auto_preamp = false
preset = "Sennheiser HD 650"
parametric = [{ type = "lowshelf", freq = 105.0, gain = 6.7, q = 0.7 }]

[eq.devices]             # output device to the preset it gets
"alsa_output.usb-Sennheiser-00.analog-stereo" = "HD 650"

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
api_secret = ""          # its secret, which scrobbling signs with
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
name = "Home"                       # shown in the sidebar
url = "https://music.example.com"
user = "me"
kind = "subsonic"                   # subsonic or ampache; Settings finds out
stream = "raw"                      # raw (the original file), opus or mp3
bitrate = 192                       # kbps, for opus and mp3

[[server]]
name = "NAS"                        # a DLNA server, as Find DLNA servers adds it
url = "http://192.168.1.5:8200/rootDesc.xml"   # its description address
kind = "dlna"
udn = "uuid:4d696e69-444c-164e-9d41-363da121361f"   # its id, to find it again
```

Settings, Radio and Podcasts edit this file for you, and keep the rest of it as you wrote it. Server passwords are never in it; they're in your keyring.

A typo in a key name is an error, not a silent default, so you find out right away. Command line flags (`--backend`, `--device`, `--buffer-ms`, `--volume`, `--fade-ms`) override the file for one run.

Where everything else goes:

| What | Where |
|---|---|
| Settings | `~/.config/patinaplayer/config.toml` |
| Library index, playlists, play counts, podcast positions, loudness measurements | `~/.local/share/patinaplayer/library.db` |
| Downloaded podcast episodes | `~/.local/share/patinaplayer/offline/podcasts/` |
| Songs pinned for offline | `~/.local/share/patinaplayer/offline/tracks/` |
| Cached and prefetched songs | `~/.cache/patinaplayer/tracks/` |
| EQ presets you saved or imported | `~/.config/patinaplayer/presets/eq/` |
| Resized cover art, local and from servers | `~/.cache/patinaplayer/art/` |
| Server passwords, the Last.fm session | your keyring, or `~/.config/patinaplayer/secrets.toml` (only you can read it) when there's no keyring |
| Scrobbles waiting to be sent, loved songs | in the library index |

The index lives in `~/.local/share` on purpose, since cleaners like BleachBit wipe `~/.cache`, and nobody should lose their playlists to that. The art cache and the song cache are safe to wipe. They refill as you play.

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
| f / F | Keep the selection / the whole view offline, or stop |
| X | Mix from the selection: an artist, album, song, or the marked genres |
| M | Genres: mark a genre for a mix. Queue: save the mix playing |
| q | Queue panel |
| l | Love the song playing |
| Ctrl+E | The EQ view |
| . | Jump to the playing track in Queue |
| F5 | Check folders for changes and refresh podcast feeds |
| ? | Help |
| Ctrl+Q | Quit |

In Radio: `N` new station, `E` edit, `T` test, `D` delete, `J`/`K` move. In Podcasts: `N` subscribe, `D` unsubscribe. In a show's episodes: Enter plays (resuming), `w` downloads (or cancels), `x` deletes the downloaded file, `P` toggles played.

In the Queue: `d` remove, `J`/`K` move down or up, `S` add the queue to a playlist (new or existing), `L` load the queue saved on your server, `C` clear. In Settings: Enter flips, cycles or edits the option, `h`/`l` step it down or up, `N` adds a library folder, `d` removes a folder or a server. On a server's row, Enter syncs it now and `E` edits it (renames it, for a DLNA server). On Find DLNA servers, Enter searches the network and shows what it found; Enter on one adds it. In Playlists: `N` new, `R` rename, `D` delete, and inside a playlist `d` and `J`/`K`. In the EQ: Enter flips, cycles or edits, `h`/`l` move a band half a dB, `H`/`L` move a parametric band's frequency, `o` turns the EQ on or off, `d` removes a parametric band. In Downloads: Enter opens a pinned item, `w` pauses or resumes a download, `x` cancels it or unpins, `W` pauses all downloads, `C` clears the cache.

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
    src/remote/         music servers: the Subsonic, Ampache and DLNA clients, syncing into the index, the server threads
    src/cache/          the song cache: downloads, reading a song while it downloads, prefetch, pins, eviction
    src/mix.rs          genre, artist and track mixes, and who's similar
    src/level/          leveling: picking the gain, EBU R128 measuring, the live normalizer, the measuring thread
    src/dsp/            the real-time stages: biquad filters, the EQ as it runs, the true-peak limiter
    src/eq/             the EQ as you set it: graphic and parametric bands, presets, AutoEq and APO import
    src/lastfm.rs       Last.fm calls: signing, linking, Now Playing, scrobbles, love, similar artists and songs
    src/scrobble.rs     the Last.fm thread: linking, the scrobble queue, retries
    src/secrets.rs      passwords in the keyring (Secret Service over D-Bus), or a private file
    src/config.rs       config file and XDG paths
    src/player.rs       core thread: commands in, snapshots and events out
    src/queue.rs        queue on stable entry ids, shuffle, repeat
    examples/scan.rs    time an index of your own folders
    examples/mock_server.rs  a stand-in Navidrome on localhost, to try the apps without a server
    tests/              gapless, seek, library, watcher, player, filter, network, server, keyring and browser tests
  patina-tui/           the terminal frontend (ratatui, ratatui-image)
  patina-gui/           the desktop app (Slint)
    ui/app.slint        the whole window
    ui/fonts/           bundled fonts and their license
    src/bridge.rs       turns the shared browser into Slint models and back
assets/                 app icon and desktop file
packaging/              AppImage: FFmpeg build, AppDir, entry point
.github/workflows/      CI on every push, real servers in containers, AppImage on every tag
```

The core owns all the state. The views and what each action does live in `patina-core/src/browse`, so the TUI and the desktop app are only drawing and input. A fix to how a view behaves lands in both at once.

The library runs on its own thread with its own database connection. SQLite is in WAL mode, so the views keep reading while a scan writes. Paths are stored as raw bytes, since Linux file names don't have to be valid UTF-8.

The queue refers to entries by id, never by position. That's what lets you reorder or remove tracks while the decoder is already reading the next one, without anything getting confused.

To see how fast indexing is on your own collection: `cargo run --release -p patina-core --example scan -- ~/Music`. It uses a throwaway database, so your real index isn't touched.

The bit-perfect test plays a 16/44.1 and a 24/96 FLAC of white noise to ALSA's `file` plugin, which writes everything it's sent to a file, then compares that file byte for byte with FFmpeg's own decode. Any resampling, volume, dither or padding anywhere in the chain fails it. No sound card needed.

Server songs live in the same index as local files, under made-up paths like `server://Home/Nirvana/Bleach/01 Blew.flac` that follow the server's own folder layout. That's what makes every view, search and the folder tree work the same for both. The server's song id sits next to the path, and that's what playing, playlists and covers use. A song's stream address carries a login token, so it's asked for when the song starts rather than stored.

The server tests run stand-in servers inside the test itself: one that answers like Navidrome (OpenSubsonic fields, made-up paths), one like Gonic (prefixed ids, real paths, missing fields, one-item lists sent as a lone object) and an Ampache one whose sessions run out every few calls. `.github/workflows/servers.yml` then runs the same clients against real Navidrome, Gonic and Ampache containers. The keyring test runs its own small Secret Service on a private D-Bus, locked keyring and unlock prompt included.

The mix tests check genre mixes against two stand-in servers holding the same songs (each plays once, and with one server away the songs come from the other), artist mixes from the server's similar artists, track mixes from the server, then a stand-in Last.fm, then shared genres, endless refills, and saved mixes.

The cache tests play songs through the cache from the stand-in Navidrome, and check prefetch by size and by count, eviction least recently played first, pins moving between the cache and the offline folder, converted downloads, podcast episodes in the same queue, and what greys out with a server away. One decodes a FLAC while it's still being written and checks it comes out sample for sample the same as the finished file. Another stalls a download and checks that a read near the end goes to the server by range. The servers workflow runs the cache against real MiniDLNA and Gerbera as well.

The DLNA tests run a stand-in media server that answers SSDP searches on a private address and ContentDirectory calls three ways: a folder view that titles songs by file name (like Gerbera), search only (like Jellyfin, with the media server nested in another device and a URLBase), and neither, with every song under three views. It also lists a converted MP3 before each original FLAC and hands out two items per page, so the paging and the pick of the original both get used. The servers workflow then runs the same client against real MiniDLNA and Gerbera.

The leveling tests measure FFmpeg-made sine tones into a real index and play them through the player with a capturing output: two songs 6 dB apart come out level in Track mode and still 6 dB apart in Smart mode in album order, back to level on shuffle; an unknown song comes out 6 dB down; a podcast 12 dB too loud is ridden down to the target by the live normalizer. The cache test checks that songs landing in the cache from a stand-in Gonic get measured, and that a stand-in Navidrome's replayGain field is used as it is.

The EQ tests play a 1 kHz tone through the player with a capturing output: a +6 dB band at 1 kHz comes out exactly twice as loud, a bypassed EQ comes out bit for bit, and a cut far away leaves it alone. The limiter test drives a tone 9 dB into the EQ and checks nothing passes -1 dBTP, that a lower ceiling holds, and that with the limiter off it's clipped at full scale rather than wrapped. Others drive the EQ view's rows, save and load presets, import AutoEq files, give the output its own EQ and check what config.toml holds after. One test runs against a real PipeWire with two outputs and switches the default between them; the CI starts a private PipeWire for it.

The Last.fm tests run a stand-in Last.fm that checks every call's signature. They link through the approval page (waiting while it isn't approved yet), send scrobbles while it's down and check they arrive later in batches of 50 with their original times, drop a session it stops accepting while keeping the queue, love a song and ask if it's loved, and play a 32-second song in real time to see Now Playing at the start and the scrobble at the halfway mark, timed from when it started. Another checks, for each choice of who scrobbles, that a server song goes to Last.fm, to the stand-in Navidrome's scrobble endpoint, or both, exactly once each.

To try the server side without a server: `cargo run -p patina-core --example mock_server` starts the stand-in Navidrome and prints its address; log in as `demo` with password `demo`.

The network tests run a small HTTP server inside the test itself: an ICY stream that hangs up on purpose to check reconnects, a `.pls` with a dead first mirror, real HLS segments, an RSS feed, and a download that breaks halfway to check resume. No internet needed.

## Known gaps in this milestone

- Podcasts and stations have no art yet. Podcast feed images and station logos come later, cover art for tracks works now.
- Bit-perfect was checked bit for bit here through ALSA's file plugin, at 16/44.1 and 24/96, and the PipeWire hand-off was checked against a stand-in on a private D-Bus. It hasn't touched a real DAC or a real PipeWire from this box. CI also tries a real ALSA kernel driver (snd-aloop) when the runner can load it.
- In bit-perfect mode, going between tracks of different formats (16/44.1 into 24/96) closes and reopens the card, so there's a short gap. Same-format tracks are gapless as always.
- The Browse folder picker goes through the desktop portal (xdg-desktop-portal), which Plasma, GNOME and most desktops run. Without it, Browse says so and Type a path still works.
- Command line flags like `--backend` still override the file for one run, but Settings shows the file's values, not the override.
- No tray icon and no MPRIS media keys yet. Those are milestone 15.
- The kawaii fonts are bundled as subsets (Latin, kana and symbols), which saves about 3 MB per weight. Kanji in your own tags still show through your system's Japanese font, so a Japanese title can mix two typefaces. With no Japanese font installed at all, kanji show as boxes in every theme, the same as before.
- The themes were checked under X11 (Xvfb) here, at full width and narrow, like the rest of the desktop app.
- The AppImage build was run end to end on Ubuntu 24.04 here, the same system as the GitHub runner, but the workflow itself hasn't run on GitHub yet. The first tagged run is the real test.
- The desktop app was checked under X11 (Xvfb) here, not on a real Plasma Wayland session. Slint supports Wayland natively, so it should be fine, but that's untested from this box.
- When the next track has a different sample rate (44.1 kHz into 48 kHz, say), the output reopens at the new rate and there's a short gap. Gapless across rates needs the resampler, which is milestone 12.
- Built from source the normal way, both binaries link your system's FFmpeg, PipeWire and ALSA. The AppImage links its own FFmpeg in.
- 16-bit ALSA output is rounded, not dithered. TPDF dither lands with the output controls in milestone 12, and it only matters on the ALSA fallback with a device that refuses float and 32-bit.
- Graphic bands are peak filters, and neighbours overlap, so two adjacent bands at +6 dB make a bump a little over 6 dB between them. The curve shows what you get. Imports fitted to the 31-band EQ come within about a dB of the AutoEq curve in the tests, and a curve past ±12 dB stops at 12.
- The EQ's filters are recalculated on every change and keep their memory, without a crossfade between old and new settings. A very fast drag of a big boost can make a faint zipper sound while you drag. Bypass and on do crossfade.
- Per-device EQ was tested here against a real PipeWire 1.0 with two stand-in outputs (null sinks), switching the default between them, and the CI runs the same. It hasn't met real headphones, Bluetooth or HDMI outputs from this box. Bluetooth headsets that change profile (A2DP to headset mode) show up as a different device, so they'd want an EQ each.
- The limiter's oversampling uses an 8-tap interpolator. Peaks between samples are caught to within a few tenths of a dB at the very top of the audio band; the samples themselves never go over the ceiling.
- The GUI's Import button uses the desktop's file picker. Without one (no xdg-desktop-portal), it asks for the file's path instead.
- An album gain worked out from measured songs averages their loudness by energy, weighted by length. That's the R128 album figure without the gating across songs, so on an album with long near-silent stretches it can differ from a tagger's by a few tenths of a dB. Album gains in tags are used as they are.
- Opus files tagged with R128_TRACK_GAIN (opusgain style) and no REPLAYGAIN tags get measured rather than read.
- A song that changed on disk (a new size) is measured again. A server song that changed on the server keeps its old measurement until its cached copy is dropped and fetched again.
- Measuring decodes the whole song once, on its own thread. Measure now goes a song at a time with a short rest between, so it can run while you listen, but a big library takes a while.
- After a leveling change in Settings, the couple of seconds already decoded play out at the old gain first.
- The server clients were tested here against stand-in servers built to the Subsonic, OpenSubsonic and Ampache API documents, with the quirks Navidrome and Gonic are known for. They haven't met a real server from this box, which can't reach the internet or run containers. The servers workflow runs them against real Navidrome and Gonic on GitHub; it hasn't run yet, so its first run is the real test. Its Ampache job sets Ampache up through Ampache's command-line installer, which I couldn't try here, so that job reports failures without failing the run.
- DLNA was tested here against real MiniDLNA 1.3.3 and Gerbera 2.0 (found on the network, synced, covers, a song played through), and against the stand-in above. Jellyfin, Plex, Serviio and Twonky haven't been tried. The search and walk paths are there for the way they lay out their trees, but their first real run is yours. Jellyfin 10.9 and later ship DLNA as a plugin, so install it from the plugin catalogue first. Plex has its own DLNA switch in its settings.
- DLNA servers have no playlists, play queue, stations or podcasts PatinaPlayer can reach, so a DLNA source is songs only. Its songs still go in your local playlists.
- Folder paths on a DLNA server come from its folder view, and the file names are made from the track number and title, since DLNA doesn't send file names. On a server with no folder view they're Artist/Album/01 - Title, like Navidrome's.
- Finding servers needs multicast on the local network. A server on another subnet or behind a VPN won't answer. Its description address (the `rootDesc.xml` or `description.xml` URL) can go in config.toml by hand as a `kind = "dlna"` server.
- Gerbera only listens on the machine's network address, not on localhost, so on the machine it runs on it's found under that address.
- The Light, Dark and OLED themes were checked under Xvfb like the rest. OLED was only looked at on a normal screen here, so how much power it saves on yours is your screen's business, but every background in it is #000000.
- The cache was tested here against the stand-in servers and against real MiniDLNA 1.3.3: prefetched songs came out byte for byte the same as the server's files, and a song played from its download. It hasn't met a real Navidrome, Gonic or Ampache from this box. The servers workflow runs the cache against MiniDLNA and Gerbera on GitHub.
- A song downloading as a conversion (Opus or MP3 from the server) has no size until it's done, so seeking past what's in waits for the download to get there. The original file seeks straight away.
- One download runs at a time, so the most urgent one gets the whole line. A big pinned library on a slow line takes a while, and the queue in Downloads shows where it's at.
- Prefetch plans with the sizes the server reports. A DLNA server that reports none gets planned at 30 MB a song, so by-size prefetch may hold more or fewer songs than the setting suggests.
- Metered connections are only seen through NetworkManager, which Plasma and GNOME use. Without it, nothing counts as metered.
- An album pin follows the album's name and artist. If the album is retagged on the server, the pin stops matching and its songs drop back into the cache. Pin it again.
- Genres can't be pinned, since a genre is usually most of a library. Save a genre mix and keep that offline instead.
- Mixes were tested against stand-in servers and a stand-in Last.fm. Real similarity data hasn't been tried from this box, which can't reach Last.fm. Navidrome only has similar artists and songs when its Last.fm agent is set up (`LastFM.ApiKey`); without it, mixes fall back to Last.fm here (with a key) or your tags.
- Matching Last.fm's answers to your library goes by artist and title, exactly as tagged apart from case. "The Beatles" and "Beatles" don't match, and neither do titles with "(Remastered)" on one side.
- The last 100 picks are remembered while the app runs, not between runs.
- Genre mixes come from the index. Before a server's first sync finishes, its songs aren't in mixes yet.
- A Last.fm API key and secret go in config.toml in the clear. They identify the app, not you: someone holding them can make calls as the app, never as your account. Your account's session key goes in the keyring.
- Last.fm was tested here against the stand-in above, which follows Last.fm's API documents. This box can't reach Last.fm, so the first real link and scrobble are yours. The builds published from GitHub only have a built-in key if the repository's secrets are set; otherwise put your own in Settings.
- Radio scrobbling was tested on its title parsing ("Artist - Title" in, anything else ignored), not end to end against a live station.
- Ampache gets finished plays through its `record_play` call, which needs Ampache 5 or later; it has no Now Playing. DLNA servers can't be told about plays at all.
- Server songs that aren't kept only play while the server can be reached.
- No ratings, lyrics or Favorites yet, and starring isn't synced. Those are milestone 14.
- Folders for a server follow the paths the server reports. Navidrome makes those up from the tags (Artist/Album/01 - Title.flac) unless its `Subsonic.DefaultReportRealPath` option is on, so a Navidrome folder tree looks like that. Gonic reports the real paths. Ampache's are made relative to its catalog folder.
- A tag edit on the server that leaves an album's song count, length and dates the same isn't seen by the half-hourly sync. Sync now in Settings reads every album and catches it.
- Navidrome doesn't do podcasts, so its Podcasts view stays your own feeds. Gonic, Airsonic and Ampache share theirs. Only episodes the server has already downloaded are listed.
- Ampache's API has no saved play queue, so Queue from only shows Subsonic servers.
- Podcast position at a sped-up or trimmed speed is tracked from markers the decoder sends a few times a second, so the saved spot can be off by a fraction of a second. Resume backs up five seconds anyway.
- The radio and podcast code was tested against local servers, including a real HLS stream made by FFmpeg. It hasn't been run against live stations from here, since this build box has no general internet access. HTTPS streams go through FFmpeg's own TLS, so a source build needs an FFmpeg built with it (distro packages are). The AppImage's FFmpeg has GnuTLS.
- inotify needs one watch per folder. A very large library can run past `fs.inotify.max_user_watches`. If it does, you'll see an error saying so, and raising the limit with `sysctl` fixes it.

## License

GPL-3.0-or-later. See `LICENSE`.

The bundled fonts (Inter, Noto Serif, JetBrains Mono, M PLUS Rounded 1c, Zen Maru Gothic) are under the SIL Open Font License, in `crates/patina-gui/ui/fonts/`.

Now PatinaPlayer plays your servers and folders through a cache that downloads ahead and holds what you've pinned, makes genre mixes and endless artist and track mixes out of all of it, and everything is working as it should be.
