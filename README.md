# PatinaPlayer

PatinaPlayer is a Linux music player for Subsonic, Ampache and DLNA servers and local folders, for people who want a minimal interface on top of a serious playback chain.

This is milestone 16 of the build plan: everything from milestones 1 to 15 (the audio engine, the library, radio, podcasts, both apps, cover art, listening stats, the AppImage, bit-perfect output, an editable Settings view, playlists from any view, the themes, Subsonic, Ampache and DLNA servers, the song cache with prefetch, offline pins and offline mode, mixes, volume leveling, the EQ and true-peak limiter, Last.fm, the output controls, the amp stack, ratings, Favorites, lyrics, media keys, the tray, the channel tools and the phone-size window), plus the Android app. Milestone 15 brought media keys and the desktop's media controls through MPRIS2, a tray icon with close-to-tray, the channel tools (mono, width, crossfeed, balance), and a desktop window that shrinks down to phone size without cutting anything off. There are two programs on one core. `patinaplayer` is the desktop app and `patinaplayer-tui` runs in a terminal. Both show the same views, play through the same engine, and read and write the same library, playlists and config. The Android app is the desktop app's window and core on a phone, with AAudio for output and bit-perfect USB on Android 14 and later.

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
- Changes are heard within one output block (a few milliseconds) and saved half a second after you stop, to `[eq]` in config.toml. The filters glide to a new setting over 30 ms instead of jumping, so dragging a band fast doesn't zipper.
- The bottom bar's EQ chip is lit while the EQ is on and changing something.

**Limiter**
- A true-peak limiter last in the chain, after the EQ and the volume, so EQ boosts and leveling never clip. On by default, with a -1 dBTP ceiling (Settings, Limiter).
- It catches peaks between samples too, the ones a DAC's reconstruction filter makes, by looking at the signal 4x oversampled.
- It looks 2 ms ahead in the buffer, so it's already down when a peak arrives, and lets go over about a tenth of a second. Looking ahead adds no delay: it reads further into audio that's already decoded.
- Below the ceiling it doesn't touch a single sample. The gapless test plays through it and still matches the source bit for bit.
- While it's working, the bottom bar shows how much it's taking off (`LIM -2.2 dB`, in copper), held for a moment so you can see it.
- Bit-perfect playback skips it, along with everything else.

**Ratings**
- Songs and albums take 1 to 5 stars. Right-click one in the desktop app and pick Rate, or press `y` on it in the TUI. An open album has its stars in the header; the song playing has them on the bottom bar (click one, or `Y` in the TUI). Rating it the same again clears it.
- Songs and Albums can sort by rating, highest first (the Sort chip, or `o` in the TUI).
- Ratings sync both ways with Subsonic and Ampache servers: what you rate here goes to the song's server, and what the server says comes in on each sync. A rating you set while the server is away waits and goes when it's back, and a sync in between never undoes it.
- Local files keep their ratings in the library, by path, so a rescan keeps them.

**Favorites**
- Star a folder, an album or an artist (right-click, Add to Favorites; `*` in the TUI; or the button in an album's header) and it joins the Favorites list in the sidebar, one click away.
- Reorder and unstar right there: right-click a favorite in the desktop app, or `J`/`K` and `d` on it in the TUI sidebar.
- Starred albums and artists sync with the server's starred list (Subsonic's star, Ampache's flag), both ways. Starred folders stay local.

**Lyrics**
- Press Lyrics on the bottom bar (`Ctrl+L` in either app) and the song playing gets its lyrics in a panel on the right. Nothing is looked up until you press it, and only for that song: when the next one starts, the panel says so and waits for another press.
- Where it looks, in order: the song's own tags (USLT, LYRICS, ©lyr), an .lrc file next to it with the same name, its server (OpenSubsonic's lyrics by song, else plain Subsonic's), then LRCLIB, an open lyrics database, only if you turn it on in Settings, under Lyrics. It's off until you do.
- Synced lyrics (LRC times, from any of those) follow the song, the line playing lit up, and scroll themselves. Plain ones scroll by hand.

**Amp stack**
- A rack of old hardware you switch on and chain, for when you want a record to sound like it's coming through a warm tube amp instead of a clean DAC. Off by default. `Ctrl+A` opens it in either app, or click AMP on the bottom bar, which lights copper while the stack is working.
- Nine devices: tube preamp (soft, mostly even harmonics), tube power amp (odd harmonics and power supply sag), output transformer (thick, rounded lows, rolled-off extremes), tape deck (7.5, 15 or 30 ips: saturation, head bump, top-end loss, wow, flutter, hiss if you want it), phono stage (mono bass, rumble, crackle if you want it), console channel (a whisper of saturation and crosstalk), bus compressor (opto or VCA), exciter, and bass enhancer.
- Every device has its own on switch, input and output trims, and a mix knob where blending makes sense. Each kind goes in the rack once, in any order: move a device up or down the chain from its rows.
- Presets: Warm Glow, Reel to Reel, Late Night, Living Room Console and AM Radio to start, and your own, saved as TOML files in `~/.config/patinaplayer/presets/amp/`.
- Each output can keep its own stack, like the EQ: your speakers get Warm Glow while your headphones stay clean. "Give this device its own stack" in the Amp view.
- The desktop app shows the rack as a row of units with dials: drag a dial up or down to turn it, click a unit's light or name to switch it on or off. The TUI shows the same devices as rows with sliders, `h`/`l` to turn them.
- The devices run at 4x the song's rate by default (2x or 8x in the Amp view, up to 768 kHz), so the harmonics they make don't fold back down as aliasing. A test drives a 7 kHz tone hard and checks the fold-back that shows at 2x is gone at 8x.
- They're tuned to sound subtle at their defaults: a tone through any one device at its defaults comes out within 3 dB of where it went in, with its harmonics more than 20 dB down. Drive adds harmonics and squeezes peaks; it doesn't turn the music up.
- These are character effects built from how each kind of hardware behaves in the textbooks (a triode stage, a push-pull pentode stage, a 15 ips tape machine). They don't model any branded unit, and I had no reference gear to match them against.
- Knobs glide to where you turn them, and switching a device or the whole rack on or off fades over 20 ms, so nothing clicks. Moving a device along the chain fades the rack out, swaps the order and fades it back in, so that doesn't tick either. On with every device off, the stack passes the sound through untouched, bit for bit.
- It sits before the EQ, so a headphone correction curve in the EQ applies to the final sound, character included. Bit-perfect skips it, and Auto bit-perfect counts it as changing the sound.

**Channel tools**
- Mono, stereo width, crossfeed and balance, in Settings under Channel tools. They run after the EQ, in that order. Off by default, and one switch turns all four off at once for a quick A/B.
- Mono sums both sides, for one earbud or a speaker in the next room. Width goes from 0% (mono) through 100% (untouched) to 200%, by turning the side signal down or up.
- Crossfeed is for headphones: each ear gets a low-passed share of the other side, and loses the same share of its own, the way bs2b does it. A sound in the middle comes out exactly as it went in; only bass panned hard to one side gets pulled toward the middle. Light, Medium and Strong.
- Balance turns the far side down and never turns anything up, so it can't clip.
- Every control glides over 15 ms, so moving one while music plays never clicks. Off passes the sound through bit for bit. They're stereo only: a 5.1 song goes through untouched. The limiter looks ahead through a copy of them, so a width past 100% is caught early, like an EQ boost. Bit-perfect skips them, and Auto bit-perfect counts them as changing the sound.

**Media keys, the panel and the tray**
- Both apps offer MPRIS2 on the session bus (`org.mpris.MediaPlayer2.patinaplayer`), which is what Plasma's media widget, GNOME's media controls, the keyboard's media keys and `playerctl` all talk to. Play, pause, next, back, seeking, the position slider, volume, shuffle and repeat all work from there, and the widget shows the title, artist, album, length and cover.
- Server songs show their cover too (fetched once, then cached). Radio shows the stream's "Artist - Title" as the song, with the station as the album.
- The desktop app puts an icon in the system tray (a StatusNotifierItem, what Plasma's tray uses). Click it to show or hide the window, middle-click to play or pause, right-click for play, next, back, show and quit. The icon carries a small play or pause mark, and its tooltip says what's playing.
- Closing the window hides it to the tray and the music keeps going. Settings, Desktop has a switch for people who want close to mean quit, and one for the tray icon itself. `Ctrl+Q` always quits. With no tray to hide to (a desktop without one), closing quits.
- The TUI has no window to raise, so it tells the desktop that, and keeps playing when you detach tmux or screen.

**Output**
- Click the output on the bottom bar (or press `Ctrl+O` in either app) to pick where the sound goes: every PipeWire output there is, by name, or the ALSA devices when PipeWire isn't running. The song carries on from where it was on the new device. Settings has the same list under Output, Device.
- Sample rate: Match the source (the default) plays each album at its own rate. A fixed rate (44.1, 48, 88.2, 96, 176.4 or 192 kHz, or any rate in config.toml) resamples everything to it.
- Gapless across rates. When the next song on the same album has a different rate, it's resampled to the rate already playing, so the join stays gapless. Between albums the output reopens at the new rate instead, a short gap, and each album keeps its own rate. With a fixed rate, songs that follow on at the same rate go through one resampler that never stops, which is exactly the same as resampling the whole album in one go.
- Resampler quality: Fast is cubic interpolation, cheap, with no anti-aliasing filter. Balanced (the default) is a 128-tap sinc filter. Best is a long sinc filter run by FFT. The bottom bar shows `44.1→48k` in copper while a song is being resampled.
- The resampler's own delay is measured to a fraction of a sample and taken out, and the edges of each song are run through the filter with a mirror of the song's own first and last samples, so nothing is shifted and there's no dip at the start or end. The tests hold every quality to within 2/10,000 of the exact signal.
- If an ALSA device can't play a rate (a `hw:` device that only does 48 kHz, say), the output resamples to the rate it offers and says so in the log, instead of failing.
- Sample format, for ALSA: Auto takes the best the device accepts (float, then 32, 24 and 16-bit), or pick one. PipeWire always takes float and sets the device's format itself, so the row greys out under PipeWire.
- Dither: TPDF dither on 16 and 24-bit ALSA output, on by default. A sample that's already exact at that depth (an untouched 16-bit file at 16 bits) goes out exact; everything else gets the dither instead of plain rounding.
- Crossfade: 0 to 12 seconds, equal power. Never between songs that follow on from each other on an album, so live albums and DJ mixes stay gapless. Radio and podcasts don't crossfade, and a skip by hand starts the next song at once.
- None of these touch bit-perfect playback. Auto bit-perfect counts a fixed rate and crossfade as changing the sound, the same as leveling and the EQ.

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
- The window shrinks down to 360 by 420 pixels and steps down as it goes, so nothing gets cut off. Under 1180 pixels wide the bottom bar drops the format badge and output chip. Under 1000 the sidebar folds to icons (hover one for its name) and only one side panel opens at a time. Under 820 the bottom bar takes two rows (the song and controls, then the transport across the whole width) and a view's buttons move under its title. Under 600 the sidebar becomes a drawer behind the menu button, and the queue and lyrics panels cover the view. On a short window the album header gets smaller, and when the header, EQ or amp panel and some of the list still don't fit, the whole view scrolls; the list keeps at least a few rows of its own.
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
- Auto mode means "bit-perfect whenever nothing would change the sound": leveling Off, the EQ bypassed or flat, the amp stack off, the sample rate set to Match, and crossfade off. With leveling on (the default), that's never, so Auto is for people who set leveling to Off.
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

GitHub builds it. Push a tag and you get a release with the AppImage on it (and the APK, see The Android app):

1. Push the repo to GitHub, with `.github/workflows/appimage.yml` in it.
2. Tag a version and push the tag: `git tag v0.2.0 && git push origin v0.2.0`
3. Wait for the Release workflow under the Actions tab. It takes about 15 minutes the first time and less after, since FFmpeg and the Rust build are cached.
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

## The Android app

Same window, same views, same core as the desktop app, built for arm64 phones running Android 9 or later. GitHub builds it next to the AppImage: push a tag and the release gets `PatinaPlayer-<version>-arm64.apk` as well.

THE APK HAS TO BE SIGNED WITH THE SAME KEY EVERY RELEASE, or Android refuses to install it over the old one and you lose the app's data to reinstall it. Making that key and giving it to GitHub is a one-time job, walked through in the signing guide that came with this milestone. Until the key is in the repository's secrets, tag builds stop at the APK step on purpose, and the AppImage still ships.

Installing it:

1. Download the APK from the release on your phone.
2. Open it. Android asks to let your browser (or file manager) install apps; allow it for that one app.
3. Open PatinaPlayer and allow music and notifications when asked.

What it does on a phone:

- Your phone's Music folder (`/storage/emulated/0/Music`) goes in the library on first start. More folders go in under Settings, Library, by typing the path, since there's no folder picker yet.
- Servers, radio, podcasts, the cache, mixes, Last.fm, the EQ, the amp stack and leveling all work as on the desktop. Everything the app keeps (config, the index, the cache, cover art) lives in its own private storage.
- Music keeps playing with the screen off. The notification and the lock screen show what's playing with its cover and have back, play/pause and next. Bluetooth headphone buttons and a car's Bluetooth controls go through the same media session. Android Auto isn't supported: it needs the app to offer its library for browsing, which it doesn't yet.
- Unplugging headphones pauses. A call or a navigation prompt pauses and comes back after; another player starting stops this one.
- Back goes back through the views. With nowhere left to go, it puts the app in the background and the music keeps going.
- Output goes through AAudio. Settings, Output lists the phone's speaker, wired headphones, USB DACs, Bluetooth and HDMI as Android sees them, and follows them as they're plugged in and out.
- Bit-perfect, on Android 14 and later with a USB DAC: Android's mixer is put into bit-perfect mode at the song's rate and depth and the song goes to the DAC untouched. Other sounds on the phone don't reach the DAC while it plays, which is the same tradeoff as on the desktop. On older Android, or a DAC that Android doesn't offer bit-perfect for, it says why and plays through the mixer.
- Passwords go in `secrets.toml` in the app's private storage, since there's no desktop keyring on a phone. Other apps can't read it; anyone with root on the phone can.

Building it yourself: `bash packaging/build-apk.sh`. It needs the Android SDK with build-tools and a platform of 34 or newer (`ANDROID_HOME`), the NDK (`ANDROID_NDK_ROOT`), JDK 17 or newer, cmake, and `rustup target add aarch64-linux-android`. It builds FFmpeg and Mbed TLS for Android first (`packaging/build-ffmpeg-android.sh`), then the Rust library, the Java side and the APK, all without Gradle. The result is in `target/apk/`. Without a release key in the environment it signs with a debug key it keeps in `target/android-debug.keystore`, which is fine for trying it on your own phone.

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
rate = "match"           # or a rate in Hz, like 48000, to resample everything to
resampler = "balanced"   # fast, balanced or best
format = "auto"          # ALSA only: auto, f32, s32, s24 or s16
dither = true            # TPDF dither on 16 and 24-bit ALSA output
crossfade_ms = 0         # 0 to 12000

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

[amp]                    # the app writes this table too; the Amp view is easier
on = true
oversample = 4           # 2, 4 or 8
preset = "Warm Glow"

[[amp.units]]            # one per device, in signal order
kind = "tube-preamp"
drive = 3                # knobs by name; anything left out takes its default
bias = 35

[[amp.units]]
kind = "transformer"
on = false
trim_out = -1.5

[amp.devices]            # output device to the stack preset it gets
"alsa_output.pci-0000_00_1f.3.analog-stereo" = "Speakers stack"

[lyrics]
online = false           # ask LRCLIB when nothing local or on the server has them

[channel]
on = false               # mono, width, crossfeed and balance, after the EQ
mono = false
width = 100              # percent: 0 is mono, 200 the widest
crossfeed = "off"        # off, light, medium or strong
balance = 0              # -100 (left only) to 100 (right only)

[desktop]
tray = true              # the desktop app's tray icon
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
| Amp stack presets you saved | `~/.config/patinaplayer/presets/amp/` |
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
| Ctrl+O | Pick the output device |
| Ctrl+A | The amp stack |
| Ctrl+L | Lyrics for the song playing |
| `*` | Star the selected folder, album or artist (Favorites) |
| y / Y | Rate the selected song or album / the song playing |
| . | Jump to the playing track in Queue |
| F5 | Check folders for changes and refresh podcast feeds |
| ? | Help |
| Ctrl+Q | Quit |

In Radio: `N` new station, `E` edit, `T` test, `D` delete, `J`/`K` move. In Podcasts: `N` subscribe, `D` unsubscribe. In a show's episodes: Enter plays (resuming), `w` downloads (or cancels), `x` deletes the downloaded file, `P` toggles played.

On a favorite in the sidebar: `J`/`K` move it, `d` takes it out of Favorites, Enter opens it.

In the Queue: `d` remove, `J`/`K` move down or up, `S` add the queue to a playlist (new or existing), `L` load the queue saved on your server, `C` clear. In Settings: Enter flips, cycles or edits the option (a long list, like Device or Sample rate, opens to pick from), `h`/`l` step it down or up, `N` adds a library folder, `d` removes a folder or a server. On a server's row, Enter syncs it now and `E` edits it (renames it, for a DLNA server). On Find DLNA servers, Enter searches the network and shows what it found; Enter on one adds it. In Playlists: `N` new, `R` rename, `D` delete, and inside a playlist `d` and `J`/`K`. In the EQ: Enter flips, cycles or edits, `h`/`l` move a band half a dB, `H`/`L` move a parametric band's frequency, `o` turns the EQ on or off, `d` removes a parametric band. In Downloads: Enter opens a pinned item, `w` pauses or resumes a download, `x` cancels it or unpins, `W` pauses all downloads, `C` clears the cache.

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
    src/dsp/            the real-time stages: biquad filters, the EQ as it runs, the channel tools, the true-peak limiter, the resampler
    src/mpris.rs        MPRIS2 on the session bus, for media keys and the desktop's media controls
    src/dsp/amp/        the amp stack as it runs: the oversampler and the nine devices
    src/amp.rs          the amp stack as you set it: devices, knobs, presets, files
    src/eq/             the EQ as you set it: graphic and parametric bands, presets, AutoEq and APO import
    src/lyrics.rs       lyrics lookup: tags, .lrc files, the server, LRCLIB; LRC parsing
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
  patina-gui/           the desktop app (Slint), and the window the Android app shows
    ui/app.slint        the whole window
    ui/fonts/           bundled fonts and their license
    src/bridge.rs       turns the shared browser into Slint models and back
  patina-android/       the Android app's native library: startup, the JNI glue, the media session's feed
android/                manifest, icons and the Java side (permissions, media session, device list, bit-perfect USB)
assets/                 app icon and desktop file
packaging/              AppImage and APK builds, FFmpeg for each
.github/workflows/      CI on every push, real servers in containers, AppImage and APK on every tag
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

The marks tests rate songs and albums through the shared browser on a real indexed folder and check the rating sorts, the album header and the song playing; star a folder, an album and an artist, reorder them, open one from the sidebar and unstar them; and look up a song's lyrics from its tags, synced. Against the stand-in Navidrome, ratings and the starred list come in on a sync, a rating made here survives a sync while it waits and then reaches the server, a rating taken away is taken away there too, and unstar and star go out both ways. Others get synced lyrics from the stand-in's OpenSubsonic endpoint through the same lookup the button uses, check a stand-in LRCLIB is never asked while it's off and is asked once it's on, and check the stand-in Ampache hears rate and flag.

The amp tests play a tone through the player with the stack off and on: on with every device off comes out bit for bit, a driven tube preamp keeps the tone and adds a second harmonic that wasn't there, and every built-in preset plays without going near clipping. Unit tests check each device at its defaults stays within 3 dB with its harmonics 20 dB down, that the tube preamp's second harmonic leads and the power amp's third does, the compressor's gain at 4:1, the phono stage folding the bass to mono, that switching a device on glides in without a jump, and that the oversampler is a clean delay that keeps images 60 dB down. Others build a rack from the Amp view's rows, save and load presets, give the output its own stack, and check what config.toml holds after.

The output tests play FFmpeg-made tones through the player with a capturing output and compare them with the same tone worked out exactly. Three 44.1 kHz parts of an album cut at odd places come out at a fixed 48 kHz through one output, every frame there, within 3/10,000 of the exact tone across the joins. An album that goes from 44.1 to 48 kHz between tracks stays on one output with no gap and no click at the join. Two albums at different rates each get their own rate. A one-second crossfade between two steady levels is checked at the start, quarter points and end of the fade against the cos and sin curve, and two tracks that follow on on an album are checked untouched with crossfade on. Unit tests check every resampler quality for pitch, length, level, lining up to a fraction of a sample, and that Balanced and Best keep a 30 kHz tone out of a 44.1 kHz output where Fast lets it fold down. The dither test checks it averages to the true value with TPDF's noise level and keeps a tone a third of a step tall that rounding wipes out. One more lists a real PipeWire's outputs.

To try the server side without a server: `cargo run -p patina-core --example mock_server` starts the stand-in Navidrome and prints its address; log in as `demo` with password `demo`.

The network tests run a small HTTP server inside the test itself: an ICY stream that hangs up on purpose to check reconnects, a `.pls` with a dead first mirror, real HLS segments, an RSS feed, and a download that breaks halfway to check resume. No internet needed.

## Known gaps in this milestone

- The Android app was type-checked and linted here against Android's own API (the Rust side for aarch64-linux-android with its standard library built from source, the Java side against `android.jar`), and the APK assembly, signing and verification were run end to end with stand-ins for the NDK compiler and d8. It hasn't been linked with the real NDK, run on a phone or an emulator, or built by the workflow yet. This box can't reach Google's servers for the SDK. The first tagged run, and the first install, are the real tests.
- The APK is arm64 only. That's every phone from the last several years; there's no x86_64 build for emulators on a PC.
- Without the music permission, the Music folder scans as empty, and it's scanned again the moment the permission is granted. If the folder watcher couldn't start then, new files turn up on the next start or with Rescan rather than straight away.
- There's no folder picker on Android yet. Folders beyond Music go in by typing their path, like `/storage/emulated/0/Download/Albums`. A memory card's path has its own id in it, `/storage/1A2B-3C4D/Music`, which a file manager shows.
- AutoEq import on Android needs the file's path typed in too, for the same reason.
- Bit-perfect on Android was written against the Android 14 API documents, not tried on a DAC. What a phone offers depends on its maker: some don't offer bit-perfect mode at all, and then the app says so and plays through the mixer.
- FFmpeg on Android uses Mbed TLS for HTTPS streams instead of GnuTLS. Like the desktop build, it doesn't check stream certificates. Server, Last.fm and lyrics calls go through the app's own HTTP client, which checks them against Android's system certificates. Certificates you installed yourself on the phone aren't trusted by it, so a server behind a private CA needs plain http on the home network (allowed) or a real certificate.
- The release workflow is now one workflow, still in `appimage.yml`, named Release: it builds the AppImage and the APK side by side, then makes one release with both. If one build fails, the other still ships.
- Podcasts and stations have no art. Cover art for songs works everywhere, MPRIS included.
- MPRIS was tested here against a private D-Bus with the calls Plasma's widget and `playerctl` make (play, pause, next, the position slider, loop, shuffle, volume, raise, quit) and the change signals they listen for. The tray was tested against a stand-in StatusNotifierWatcher on the same bus: it registers, its tooltip and icon follow the player, click hides and shows the window, and the close button hides to it. Neither has met a real Plasma panel from this box.
- MPRIS reports a playback rate of 1.0 even when a podcast plays sped up, and doesn't offer to change it, since speed is a podcast-only setting here.
- With both apps running, the second one to start takes `org.mpris.MediaPlayer2.patinaplayer.instance<pid>`, as the spec asks, so both show in the media widget.
- A command line flag like `--backend` overrides the file for that run only. Its Settings row shows the file's value and says which flag is in charge, "Output (--backend null this run)".
- Crossfeed is a simple one-pole crossfeed in the bs2b style, not a measured head model. Width and balance work on the left and right channels only; anything with more than two channels passes the channel tools untouched.
- Bit-perfect was checked bit for bit here through ALSA's file plugin, at 16/44.1 and 24/96, and the PipeWire hand-off was checked against a stand-in on a private D-Bus. It hasn't touched a real DAC or a real PipeWire from this box. CI also tries a real ALSA kernel driver (snd-aloop) when the runner can load it.
- In bit-perfect mode, going between tracks of different formats (16/44.1 into 24/96) closes and reopens the card, so there's a short gap. Same-format tracks are gapless as always.
- The Browse folder picker goes through the desktop portal (xdg-desktop-portal), which Plasma, GNOME and most desktops run. Without it, Browse says so and Type a path still works.
- The kawaii fonts are bundled as subsets (Latin, kana and symbols), which saves about 3 MB per weight. Kanji in your own tags still show through your system's Japanese font, so a Japanese title can mix two typefaces. With no Japanese font installed at all, kanji show as boxes in every theme, the same as before.
- The themes were checked under X11 (Xvfb) here, at full width and narrow, like the rest of the desktop app.
- The AppImage build was run end to end on Ubuntu 24.04 here, the same system as the GitHub runner, but the workflow itself hasn't run on GitHub yet. The first tagged run is the real test.
- The desktop app was checked under X11 (Xvfb) here, not on a real Plasma Wayland session. Slint supports Wayland natively, so it should be fine, but that's untested from this box. The size tiers were checked at 1360, 1100, 900, 700, 400 and 360 pixels wide and down to 420 high.
- Between albums at different rates (with the rate on Match), the output reopens at the new rate and there's a short gap. That's on purpose: each album plays at its own rate. Set a fixed rate if you'd rather never have the gap.
- A change to crossfade applies from the next song on, since the end of the song playing is already decided. Rate, resampler, format and dither changes reopen the output at once, from where it was.
- The 24-bit ALSA format is S24_LE, 24 bits in a 32-bit word. Devices that only take packed 3-byte 24-bit get 32-bit or 16-bit from Auto instead.
- The device list and switching were tested against a real PipeWire 1.0 with stand-in outputs (null sinks), and the ALSA list hasn't met a sound card at all (this box has none). Real USB DACs, HDMI and Bluetooth outputs haven't been tried from here.
- A song's rating changed on the server by another app comes in when that album syncs again (Subsonic has no list of rated songs to ask for, and a sync only reads albums that changed). Album ratings come in on every sync.
- An artist starred here is starred on the first server that has their songs. DLNA servers have no ratings or stars, so their songs and albums keep theirs here only.
- Ampache has no lyrics call, so lyrics for Ampache songs come from the cached file's tags or LRCLIB.
- LRCLIB was tested against a stand-in answering its API, not the real site, which this box can't reach.
- The amp stack costs CPU: on this box (a 2.8 GHz Xeon core) Warm Glow at 4x takes about 4% of one core at 44.1 kHz, and all nine devices at once about 14%. At 8x, double that. A 192 kHz song gets 4x at most, since the devices stop at 768 kHz.
- The limiter's look ahead sees the amp stack only by its trims, since the stack's state can't be borrowed for a preview. A peak the stack makes itself (an exciter at full, say) is still held under the ceiling, with a shorter ramp.
- The stack delays the sound by 48 frames (1 ms at 48 kHz) while it's on. Switching it on or off crossfades against the undelayed sound, which is a 20 ms smear, never a click.
- Crossfade needs the two songs at the same channel layout. A stereo song into a 5.1 one plays out the stereo song in full, then starts the next.
- Built from source the normal way, both binaries link your system's FFmpeg, PipeWire and ALSA. The AppImage links its own FFmpeg in.
- Graphic bands are peak filters, and neighbours overlap, so two adjacent bands at +6 dB make a bump a little over 6 dB between them. The curve shows what you get. Imports fitted to the 31-band EQ come within about a dB of the AutoEq curve in the tests, and a curve past ±12 dB stops at 12.
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

Now PatinaPlayer plays your servers and folders through a cache that downloads ahead and holds what you've pinned, on your desktop and on your phone, answers your media keys, your panel's media controls and your lock screen, and everything is working as it should be.
