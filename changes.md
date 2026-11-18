# Changes

2026.11.18 - Bug fixes (Android mouse, Android playlist files, selection bar, field list) / Feature addition (Info page, Snapcast listening, smart playlist files, artist pictures, cover search terms, terminal cast menu)

* Android, mouse: the wheel no longer crashes the app, and right-click opens the menu instead of crashing. The crash on the wheel is in Slint's Android backend, which has `todo!()` for scroll events (Slint 1.18 and 1.19 both). It also took a mouse click as a finger touching down on top of the button press, so a right click was a left press and a right press at once. `packaging/slint-android-mouse.patch` makes the wheel scroll (60 px a notch, as on the desktop), and takes a mouse's clicks only from its button events, with moves while a button is held as drags. `packaging/patch-slint-android.sh` fetches the backend at the version `Cargo.lock` has, checks its checksum, patches it and hands it to cargo with `--config` for the APK build only. Nothing pinned, nothing vendored; it follows `cargo update`.
* Android, playlist files: Import in Playlists opens the system's file picker (any file, copied in under its own name), and Export (a playlist, a list, the queue, a smart playlist's rules) goes through the system's save dialog, as the library backup does. Before, both asked for a path, which on a phone is inside the app's private folder. The save dialog's file type now comes from the extension, so a provider keeps `Mix.m3u8` as it is.
* The selection bar no longer covers the last rows: while it's up, the list (and the album grid and the queue) ends two rows short of the bottom, so the last row scrolls out from under it and can be tapped to unpick it.
* Drop-down lists (the smart playlist field list and the rest) stay open after a scroll. The popup closed on any release by default, so lifting the finger at the end of a scroll closed it; now only a tap outside does, and a choice.
* View info: in the menu of a song, an album (list or grid) and a queue row, and as an Info button on Now Playing (desktop and phone, and in the phone's top menu). It opens a panel where the lyrics go, with everything known: for a song its tags as indexed (every genre, composer, grouping, BPM, comment), rating, heart, plays, last played, date added, the format in full, size, path or server and id, ReplayGain, then every tag in the file as it is now and its pictures, for a local song or a server song in the cache. For an album its artists, years, genres, songs, discs, length, rating, plays, formats, size and folder. From Now Playing it follows the song. Back and Escape close it.
* Lyrics follow the song: with the panel open, the next song's lyrics are looked up as it starts, in the window and the terminal app. Radio gets a line saying it has none. The panel being open is the ask, so nothing is looked up unasked.
* Smart playlists to another device: Export rules (menu of a smart playlist, a button inside one, `}` in the terminal app) writes its name and rules to a `.smart.toml` file, and Import in Playlists reads it, beside M3U and PLS. Taken names get a number; rules that don't read are left out and named.
* Artist pictures: Find artist picture (Deezer's artist search) and Artist picture from an image file, in an artist's menu in the list and the grid. Saved as `artist.jpg` in the artist's folder where it can be, otherwise in the app's own folder, and it wins over the folder's and the server's.
* Cover search terms: Search for, in the cover picker, looks again with your own artist, album and year (just the artist for an artist's picture). The year narrows MusicBrainz to that year and puts that year's covers first; iTunes and MusicBrainz answers now carry the year.
* Snapcast listening: the Snapcast page lists the snapserver's streams with Listen here, which plays one on this device and keeps it in Radio as a `snapcast://host:1704/<stream>` station. PatinaPlayer connects as a snapclient (Snapcast's binary protocol), puts itself on that stream through the control API, and plays FLAC, Ogg and PCM as they come and Opus wrapped in Ogg pages. Not in step with the rooms. The listener never shows in the room list, and Listen here stops playing to that snapserver first so the sound can't loop. Test station works on them too.
* The Snapcast page moved from Sources to its own Play on group in the sidebar, since it's where music goes.
* Terminal app: Ctrl+T opens a Play on picker (this device, Chromecasts, DLNA renderers and snapservers found, or a snapserver by address), and the status bar shows where it's playing. Before, the Snapcast page was there but nothing could start playing to a device.
* Tests: a smart playlist file written and read back, a file that isn't one, imports with taken names and bad rules; Info for a song (with the file's tags) and its album; open lyrics following the next song and not opening when closed; the Snapcast listener reading a stand-in snapserver's codec header and chunks as one stream, Opus packet lengths and Ogg checksums, Opus packets from a real file rewrapped in Ogg and decoded, station URLs both ways; the stream list leaving out Patina's own and the silent one, the listener left out of the rooms, and the listener moved to its stream in a group of its own; Deezer's artist answers without the placeholder, a picked artist picture saved in the artist's folder; the years in cover search answers.

Reasons for changes: the selection bar covered the last row so it couldn't be unpicked; playlist import and M3U8 export on Android went to the app's own folder; a mouse's wheel and right-click crashed the Android app; asked for a View info page from the menu and Now Playing; the smart playlist field list closed when a scroll ended; asked to carry smart playlists between devices; the terminal app had the Snapcast page but no way to cast; Snapcast was under Sources though it's a sink, and asked whether a snapserver could be a source like a radio station; asked for lyrics to follow the song while open, a way to set an artist's picture, and to type the cover search's artist, album and year.

2026.11.15 - Bug fixes (Plex 500s on original files, a server dropping out over one)

* Original files Plex fails on with HTTP 500 now play. After the one retry, the song is asked for converted (Opus 256 kbps, or the cache's own conversion if one is set) and kept under a .opus name. The fallback from earlier today only went the other way, conversions to originals, and Direct Stream downloads the original.
* A 500 from Plex no longer takes the whole server offline. Plex's client treated any error status as "can't reach the server", so a 500 during the background library sync marked PoeticPanther offline, without a line in the log. Every song on it then failed at once with "not connected", which is the flood in the log. 500 and 501 now mean Plex answered and failed at the job; 502 to 504 still mean it's not answering. Plex GET calls that fail with a 500 get one retry 1.5 s later, as a dropped connection does.
* Going offline during a sync is now logged.
* Songs on a server that isn't connected stop playback after three in a row, with one message, instead of skipping through the queue (up to 25 at a time, and the log showed about 70 in a few seconds). They aren't marked as broken, since nothing's wrong with them.
* Tests: an original the server fails on comes down converted; a Plex 500 is retried once and isn't a server gone; a run of songs on a server that's away stops playback with one message. Two cache tests that shared a stand-in server name now have their own.

Reasons for changes: with Direct Stream, Plex songs failed with HTTP 500 and were skipped, and after a few of them every song on the server failed with "not connected" and the queue emptied.

2026.11.15 - Bug fixes and UX overhaul (Plex conversions, smart playlist groups, scrolling lists)

* Plex conversions that fail with HTTP 500 no longer skip the song. A download that fails on the server's side before any data arrives is asked for once more a second later, with a new transcode session. If a conversion fails again, the original file is downloaded instead and kept under its real name and extension, so it plays (and plays bit-perfect if that's on). The log names the song and gives Plex's reason, read from its transcode decision endpoint. Any server's conversions get the same treatment; Plex is the one that needed it.
* Download errors keep the HTTP status, so the cache can tell a server failing on its side from a bad request or a dropped connection.
* Smart playlist rows can be groups, as in iTunes: "…" on a row adds a "Match any of" (or all) line with its own rules, set in under it with a bar. That covers "and" mixed with "or" both ways, like `rating > 3 and (genre is blues or genre is jazz)` and `(a and b) or (c and d)`, and such queries saved as text now open as rows. One level deep; a group inside a group stays text.
* Drop-down lists (the smart playlist field list, and every other choice list in the window) are a scrolling list of our own instead of Slint's menu, which didn't scroll: a list taller than the screen ran off it, and its first and last choices couldn't be reached. The list stays between the system bars on a phone, opens with the current choice in view, and has taller rows on a touch screen.
* README: the limitation about smart playlists being typed rules now only applies to the terminal app.
* Tests: a conversion the server fails on once is asked again, and one it keeps failing on comes down as the original; Plex's decision codes read as its reasons; groups in the form write and read back both ways, and rows and groups come and go as they should.

Reasons for changes: some Plex songs failed with HTTP 500 and were skipped; the rules editor couldn't mix "and" with "or" like the text could; the field list didn't scroll, so its ends were out of reach on a phone.

2026.11.14 - UX overhaul (Settings on Android, open Settings groups)

* Android Settings leaves out what does nothing on a phone: the Desktop group (Tray icon, Close to tray), the iPod group (iPod sync is off on Android), the Files group (paths inside the app's private storage, which nothing on the phone can open), Sample format and Dither (AAudio always takes float), and Hold on metered connections (it asks NetworkManager, which Android doesn't have). Help text there drops "Window only, not the TUI", and Bit-perfect device says USB DAC instead of sound card. The filter is one function at the end of the Settings rows, `for_android`, so the desktop and terminal apps are unchanged.
* An open Settings group's rows are indented under its name and sit on one panel in the theme's idle-selection color (`sel-idle`), with rounded corners under the last row. Hover and selection show on the panel a step up, in the raised color, since the panel already uses the idle-selection one.
* The page height counted for an open group's header was 40 px instead of its 50; fixed where the page scrolls as one.
* Tests: Android's Settings rows without the desktop-only groups and keys and without the desktop help text, with what works on a phone still there; the last row of each open group marked for the rounded corners.

Reasons for changes: asked to remove the desktop-only settings from the Android app, hide the Files section there, indent the options in each Settings group and give the open group the idle-selection background.

2026.11.14 - Bug fixes (remote control new code, Snapcast rooms turning off)

* Remote control, New code: works on the first click. It used to stop the server and start it again on the same port, but the old server's listening thread held the port for up to 50 ms after it was told to stop, so the restart failed with "port is taken" and a second click was needed. A running server now takes the new code in place, and stopping a server waits for that thread, so turning it off and on or changing its port also works at once.
* Snapcast rooms: a room can be turned off when the snapserver's only stream is Patina's. Before, off had nowhere to move the room, said "the snapserver has no other stream to go back to", and the toggle flipped back on. Now PatinaPlayer adds a silent "Patina-idle" stream (a TCP source one port above Patina's that nothing sends to) and parks the room there. A real stream is still picked first: the one the room had, else any other the snapserver has. The room's volume and mute are left alone.
* Tests: a new code and a restart on the same port taking effect at once; the idle stream added when there's no other, reused when it's there, and passed over for a real stream.

Reasons for changes: New code claimed the port was taken and needed a second click; no Snapcast room could be turned off.

2026.11.14 - Feature addition (source scope) / Bug fixes (Plex playback, playlists, connection, Android add-by-token and Now Playing)

* A source picker at the top of the sidebar (in the drawer on Android): All sources, Local, or one server. Artists, Albums, Songs, Genres, Folders, search, smart playlists and mixes show only what's in it; the queue and playlists show everything. The pick is kept in the config (`[library] scope`). Android Auto follows it. In the terminal app `B` steps through them and the status bar shows which. Settings, Library has the same choice as Browse.
* All sources shows an album more than one source has once, from the first source in Source order (Settings, Library; `local, plex` to start, then the rest). Albums of one source never hide each other. A server that can't be reached drops to the back, so a shared album shows from a source that can play it. Mixes follow the picked source but keep every copy, since they already choose one playable copy of each song.
* Plex conversions are Opus now, the codec Plex's own players get: 96, 128, 192 or 256 kbps, in Matroska over plain HTTP, the target Plex's Chromecast profile uses. MP3 320 stays, over HLS. The labels said AAC before. The transcode request now carries the client's platform and device, without which Plex can't find a profile and won't convert at all. HLS downloads follow `#EXT-X-MAP` init segments, so fragmented MP4 streams join into a playable file.
* A server song that won't play says why instead of FFmpeg's "I/O error", whether it fails to start or breaks off partway: the download's error (the server's status, like HTTP 401), or the reader's (the download stalled, a range request failed). Connection errors now carry their cause (a refused connection, an untrusted certificate) where before they only said "error sending request", and no longer include the URL, which had the server's token in it. Every such failure is also logged as a warning, so `adb logcat` shows it.
* Plex songs as the original file: before each download the app asks Plex for the song's file again, with `checkFiles=1`. The file's key carries a timestamp that changes when Plex reads the file again, so the one saved at sync could go stale. And when Plex says it can't find or open the file on its side, the note says that and names the file, instead of the download failing with HTTP 500. The synced key is the fallback if the question itself fails.
* Plex staying connected: a missed ping is asked again on a fresh connection two seconds later before the server is called offline; a Plex call that drops halfway is tried once more (not for creating a playlist); idle connections are let go after 15 seconds, before a phone's network or the server drops them under the app; the ping asks `/identity` with the full X-Plex headers and token, and checks it's still the same server.
* Plex playlists: one that won't load no longer stops the others, and when playlists fail the reason shows as a note instead of only in the log.
* Android: Add by hand on the Plex page asks for the token too. The phone's text prompt only carried two fields, so the token never got asked for and adding failed with "a token is needed". The token field is hidden as you type.
* Android: Now Playing and every other screen keep clear of the system navigation buttons. The app now reads the system bars' insets from Android and pads by whichever is bigger, those or Slint's own safe area, so the lyrics, cast and queue row sits above the buttons.
* Tests: a Plex file Plex can't open naming itself; a failed download's reason reaching the player; the scope picking one source, all of them with each album once by order, search and smart playlists in scope, following library changes, an unknown server showing nothing, and the scope through the browser and Settings; Opus and MP3 transcode URLs with the platform; HLS init segments with the token carried.

Reasons for changes: asked for a way to browse one source or all of them together, with my suggested defaults; Plex songs played from Folders gave "IO error"; Plex playlists didn't show; the Plex server showed as not connected after a few minutes open; Plex should convert to Opus, not AAC or MP3; manual Plex add on Android had no token field; and the navigation buttons covered Now Playing's bottom row.

2026.11.14 - UX overhaul (Drive screen, folded Settings) / Feature addition (library backup)

* The Drive screen: with Minimal UX while driving on (the default) and the phone in car mode, the app shows a car-style screen in place of the usual one, the way Android Auto lays things out. The cover, the song in big type, a thick progress bar, previous and next at 92 px and play at 124 px, and the heart, shuffle and volume down and up under them. A swipe left or right anywhere on it skips. Landscape puts the cover beside the buttons. Full app shows the whole app until driving ends; a Drive button in the corner brings the Drive screen back.
* Settings is folded into its groups (Library, Appearance, Playback, Output, Bit-perfect, Leveling and the rest), all closed when it opens, each showing how many settings it holds. Tap one, or press Enter on it, to open or fold it. Open all and Fold all sit at the top. The Sources pages and the EQ and Amp views stay as they were.
* Back up the library, in Settings, Library: a copy of the library database made with SQLite's VACUUM INTO, consistent even while the app is using it. On Android it's written to the app's cache and handed to the system's save dialog (ACTION_CREATE_DOCUMENT), so it can go to Downloads, a cloud drive or a USB stick, then the scratch copy is deleted. On the desktop and in the terminal app it saves to Downloads (or the path typed) as patinaplayer-library-YYYY-MM-DD-HHMM.db.
* Tests: Settings opens folded with only groups, opening one shows its rows and only its rows, and it folds again; a backup made while the library is open opens as a library with the same songs, and a second one replaces the first.

Reasons for changes: asked for Minimal UX while driving to look like Android Auto's simple screen with big buttons and swiping, a way to back up the library database out of the app's private folder on Android, and Settings to be collapsible groups that start collapsed now that it's grown busy.

2026.11.14 - Feature addition (Plex, Icecast, Snapcast rooms, Sources pages) / Bug fixes (Android back gesture)

* Plex Media Server as a fourth kind of server, written to Plex's published API (developer.plex.tv/pms) and checked call by call against it and the community OpenAPI spec. Music libraries sync into the index like the other servers, songs stream as the original file, ratings go both ways (out of ten in Plex), playlists come across and changes go back, and Plex's smart playlists come across read-only.
* Signing in to Plex follows "Authenticating with Plex", Option 1: an Ed25519 device key, its public half sent as a JWK with a PIN, the PIN entered at plex.tv/link, then polled with a JWT signed by the key, which returns a 7-day Plex JWT. It's refreshed a day before it runs out with a signed nonce, at connect and while connected, and saved back to the keyring. A classic X-Plex-Token with an address works too, without plex.tv.
* A Plex server's addresses come from plex.tv: the one that worked last first, then local, then internet, then Plex's relay. Every request sends the X-Plex headers the docs list and X-Plex-Pms-Api-Version 1.1.1; lists are paged with both X-Plex-Container headers.
* Plays and Now Playing in Plex through the timeline: a report every ten seconds while playing, every thirty while paused, and on every pause, resume and stop, with a playback session id. Plex counts the play itself near the end of the song, as with its own players. The docs give POST; a server that only takes GET gets GET.
* Conversions through Plex's universal transcoder as HLS, the only way its docs offer it; the cache downloads the segments into one file. MP3 is asked for through the client profile, AAC otherwise.
* Plex Pass features, one switch per server and off by default: similar songs from sonic analysis (`/nearest`) for mixes, lyrics from the song's lyric stream, loudness from the audio stream's gain. Off, or without Plex Pass on the server, mixes, lyrics and leveling work as for any other server.
* Icecast servers: every live mount on the server, from its status page, plays as a station on the Icecast page and in Radio, read again every five minutes. Stream addresses use the address you typed, since Icecast's own often say localhost.
* Snapcast rooms: the Snapcast page lists the snapclients and puts PatinaPlayer in the rooms you pick, through the control protocol. A room sharing a group is moved to a group of its own first, so the rest keep playing what they were; turning it off sets it back to the stream it had. Per-room volume. Turning the first room on starts playing on that snapserver.
* Sources pages: Plex, Subsonic & Ampache, DLNA, Icecast and Snapcast each have a page in the sidebar under Sources, holding what used to be in Settings, Servers and the Snapcast server row. Servers no longer each take a sidebar row; their songs are in the views, and their folders under Folders.
* Android: Back works again. The manifest opted into predictive back with no callback registered, so from Android 13 the system swallowed the gesture: inside an album nothing happened, and at the top the app closed. Back now closes what's open, goes up a level, and at the top a swipe in from the left pulls the sidebar out (Android 14 tells the edges apart).
* Tests: a stand-in plex.tv and Plex server that check each request against the docs (headers, methods, paging, the device JWT's signature with the key from the PIN) for signing in, refreshing, finding a working address, syncing, streaming, ratings, the timeline, playlists and the Plex Pass features on and off; Icecast's status page as one mount or many; Snapcast routing through a stand-in control port; HLS playlists and their tokens; the Sources pages.

Reasons for changes: asked for Plex support with the Plex Pass features behind their own switch and everything checked against Plex's spec, Icecast as a source, per-room Snapcast control, the server setup out of Settings into its own sidebar entries, and the back gesture fixed.

2026.11.14 - Feature addition (smart playlist editor, new tag fields, rating playlists, Artists grid, Minimal UX while driving) / Bug fixes (Snapcast setup)

* The smart playlist editor is rows now, like iTunes': Match all or any, a row per rule (field, condition, value) with − and + beside it, Limit to N songs, minutes, hours, MB or GB, and the order songs are picked or sorted by. Ratings are stars, ranges two boxes, ages a number and days, weeks, months or years. The count updates as anything changes. Edit as text switches to the text form for what rows can't show, and Edit as rules goes back when they fit. On a phone each row takes two lines.
* New fields for smart playlists: BPM, composer, grouping, category, comment (read from the tags), track and disc number, bit rate, channels, size in MB and date modified. The first scan after updating reads every local file's tags again to fill the new ones in. A comment stored as DESCRIPTION, as FFmpeg writes it, counts.
* Limits by time and size: `limit 90 minutes`, `limit 2 hours`, `limit 700 mb`, `limit 4 gb`, taking songs in the playlist's order until the next wouldn't fit.
* Three smart playlists every library starts with: One Star, Mid Tier (2 and 3 stars) and Best Rated (4 and 5).
* Artists shows as a grid of big squares with the artists' pictures, like Albums. List, at the top of the view, goes back to rows.
* Snapcast: picking a snapserver now sets it up. PatinaPlayer asks it over its JSON-RPC control API (TCP 1705, or HTTP 1780) whether a TCP source is listening on the port it will use, and adds one ("Patina", server mode, 48000:16:2) with Stream.AddStream when there isn't, so it works with a snapserver in Docker that ignores the edited config. Picked while nothing plays, it's checked straight away: the note says it's ready and to set rooms to the Patina stream, or says which port didn't answer. Before, picking one while stopped did nothing at all until the next play.
* Android: Minimal UX while driving, on by default. When the phone is in car mode (Android Auto, or a driving mode that turns it on), the app switches to Minimal UX by itself and back after.
* Tests: the form writing queries that read back as the same rows, any and all, half-filled rows, quoting, rules that stay text; time and size limits; the new fields from real files; the rating playlists; finding a snapserver's TCP source in its status, and adding one through a stand-in control port.

Reasons for changes: asked for an iTunes-style smart playlist editor with BPM, category and the rest, default playlists by stars, Artists as big squares like Albums, Minimal UX in driving mode, and the Snapcast stream not showing up because the snapserver in Docker uses its own config.

2026.11.14 - Bug fixes (visualizer crash, database locked at start)

* Turning on the visualizer, or opening Now Playing with one already on, crashed the app. The visualizer read the audio tap before the renderer had written to it, got a sample rate of 0, and worked out FFT bins far past the end of the spectrum. A missing rate now counts as 48 kHz, and every band's bins are kept inside the spectrum whatever the rate.
* "library database: database is locked" at start: several threads open the database at once, and each set its busy timeout only after switching to WAL, which needs a lock. The one that lost the race failed at once instead of waiting. The timeout is now set first.
* Test: the analyzer at rate 0 (the case that crashed) and at rates from 1 Hz to 384 kHz.

Reasons for changes: the visualizer crashed the app as soon as it was turned on, and it couldn't be opened again since the choice is saved; the locked database warning showed at start.

2026.11.13 - UX overhaul (Minimal UX, visualizer, backdrop over everything, readable backdrop)

* Minimal UX, in Settings, Appearance: the bottom bar drops the limiter, EQ, amp, gain, bit-perfect and format badges, and the output device chip becomes an icon button beside cast and the sleep timer, with the device's name in its tooltip. Now Playing drops the format, the output line and the sound chain, and on a phone the Sound chain menu entry and the format chip go too.
* A visualizer on Now Playing, desktop and phone: clicking or tapping the cover steps through bars, mirrored bars, an oscilloscope, a ring of rays, and the plain cover. The renderer copies what it sends out into a small lock-free ring only while one is showing; a 2048-point FFT in log-spaced bars and the leveled wave are drawn at 30 frames a second. The style is kept (`[appearance] visualizer`) and is in Settings too. A long press or right-click on the cover still opens the cover menu.
* The cover backdrop, set to show everywhere, now shows through the queue and lyrics panels, the search box and the EQ and amp panels as well as the sidebar and bottom bar, and all of them let the same amount through, so nothing sits in the theme's flat color beside it.
* The backdrop stays readable whatever the cover: the brightest tenth of the blurred cover (the darkest, on a light theme) is checked against the theme's muted text, and as much of the theme's ground as needed is laid over it. A dark cover under a dark theme is left alone.
* Tests: the scope tap's mono mix and wrap-around; a tone lighting its own bar, the bars falling in silence, a quiet wave leveled up; the backdrop veil on a white cover under light text and a black cover under dark text.

Reasons for changes: asked for a Minimal UX mode that hides the technical badges and Now Playing's sound details with the output device as its own button, a visualizer cycled by clicking the cover, the backdrop to cover the queue panel, search bar and other panels that stood out, and text that stays readable over light covers.

2026.11.13 - Feature addition (play on another device, remote control, Android Auto queue) / UX overhaul (artist pictures, cover accent, gestures, placeholders, podcast speed)

* Play on another device: a cast button on the bottom bar and on Now Playing, and a last entry in the output picker, list the Chromecasts, DLNA renderers and Snapcast servers on the network. The sound still comes from the app's own player, after the EQ, the amp stack and leveling, so the queue, gapless and crossfades carry on. Chromecasts and DLNA renderers fetch a live 48 kHz WAV stream from a small HTTP server in the app; a snapserver gets the same PCM over TCP. Songs at other rates are resampled to 48 kHz on the way, and bit-perfect is off while it plays elsewhere. If the device fails, the player comes back to this one rather than trying it again on the next song.
* A snapserver that doesn't announce itself can be typed in, from the same list or Settings, Other devices (`[cast] snapcast`).
* Remote control: with it on (Settings, Other devices), PatinaPlayer on a phone or another computer can steer this one: the song and its cover, play, pause, skip, seek, volume, mute, shuffle, repeat, the heart, and the queue, where tapping a song plays it. It needs the six-digit code shown in Settings; ten wrong codes in a minute lock everyone out for the rest of it. The player announces itself with mDNS, so the other side lists it by machine name, and the code from last time is kept.
* On the controlling side, Control another player opens the other player's Now Playing, in the phone layout, over everything. Back or Escape closes it.
* Android Auto has a queue screen: the song playing, the five before it and the next ninety-five, with covers. Tapping one plays it.
* Artist pictures in Artists, round: from the artist's folder (`artist.jpg`, `band.jpg`, `photo.jpg`, or `folder.jpg` above the albums), or from a Subsonic or Ampache server. Last.fm's grey star placeholder, which Subsonic servers pass on, counts as no picture. An artist with none is remembered for a week. Android Auto's Artists tab shows them too.
* Cover accent (Settings, Appearance): the play button, seek and volume bars and Now Playing's buttons take a strong color from the playing cover, picked by saturation and brightness and kept readable against the theme. A grey cover keeps the theme's color.
* Phone gestures: swipe up on the bottom bar opens Now Playing, swipe down anywhere on Now Playing closes it (the page follows the finger), and swiping the cover left or right skips to the next or previous song.
* A section opened for the first time shows placeholder rows for a frame while it's read, instead of the window sitting still on the click. Covers still loading shimmer rather than showing the "no cover" mark. The app also comes up a little sooner, since the first section is read after the window shows.
* Podcast speed and Skip silence: the speed chip on the bottom bar still steps through the common speeds on a click, and right-click or a long press opens every speed and Skip silence. On a phone the chip is on Now Playing, beside the format, and a tap opens the menu.
* Tests: a song played to a stand-in snapserver arrives in real time at 48 kHz (checked by its zero crossings) and the connection closes on the way back; the live WAV stream's header, a listener getting the blocks, and anything but the stream being refused; a DLNA renderer description and its escaped metadata; the remote control end to end (wrong code, pause, next, the queue, picking a song, volume, the heart); deferred section builds; artist pictures from the artist's folder.

Reasons for changes: asked for artist pictures, an accent color from the cover, phone gestures, loading placeholders, a queue in Android Auto, playing to a Chromecast, a DLNA renderer or Snapcast, controlling the desktop from the phone, and podcast speed and silence skipping where they were missing. No tag editing, as asked.

2026.11.13 - Android Auto fixes

* Album covers show in Android Auto's lists and grids. The rows had no pictures at all, which a grid draws as its broken-picture sign. Covers now go to the car through a small content provider (`ArtProvider`), the only way Android Auto takes them, made at 320 pixels from the same caches the window uses. An album with no cover gets a plain tile rather than the broken-picture sign.
* The other rows have icons: Shuffle everything, Favorites, Recently played, Recently added, Mixes, playlists, smart playlists, artists and folders.
* The Albums tab is a list, each album with its cover beside it, or with one folder per letter on a big library; a letter, Recently added and an artist's albums open as a grid of covers.
* The heart and shuffle buttons carry Media3's icon numbers, so newer Android Auto draws its own heart and shuffle icons for them. It drew a blank square and a circle in their place. The app's own icons stay for older versions.
* The provider serves only covers the car was handed in a list, by a key, never by a path, so nothing else on the phone can use it to read other files.

Reasons for changes: in a real car, album covers didn't show in Albums and the heart and shuffle buttons worked but showed the wrong icons.

2026.11.13 - UX overhaul (phone Now Playing, cover backdrop, chosen covers) / Backend overhaul (Android Auto, speed on big libraries)

* Now Playing on a phone takes the whole screen and keeps to what matters: the cover big, the song and artist with the heart, the seek bar with the format between the times, shuffle, back, a wide play button, next and repeat, a volume slider, and lyrics and the queue at the foot. Rating, Go to album, Go to artist, the cover and the sound chain are in the menu at the top. The wide window's Now Playing is as it was.
* Cover backdrop: the playing song's cover, blurred and faded down into the theme's own ground, behind Now Playing or behind every view. It goes over whichever theme is on. Settings, Appearance: Off (the default), Now Playing or Everywhere, and a strength from 10 to 100%. Everywhere lets a little of it through the sidebar and the bottom bar too.
* Find cover art and Cover from an image file, on albums, folders and songs (right-click, or long press on a phone), on the album header's cover and on Now Playing's cover. Find asks iTunes, Deezer and MusicBrainz (Cover Art Archive) and shows what they have as tiles to tap; the picture is fetched at its biggest. Either way it's saved as `album_art.jpg` or `album_art.png` in the album's folder, and the rows, the header, Now Playing and the backdrop show it straight away.
* A cover you chose wins over art embedded in the files. A server album's folder, or a local one the app can't write to (most folders on Android, where it can only read music), keeps the cover in the app's own folder, filed under the album's folder path.
* Android Auto starts without the app: connecting the car with the app closed starts the player in the background, with the last queue where it was. Before, the car could only say "Open PatinaPlayer on the phone". Opening the app afterwards takes over the same player, song and all, rather than starting a second one.
* Android Auto has a heart for the song playing and a shuffle button beside the transport, each drawn as it is now.
* Android Auto's tabs: Home (Shuffle everything, Favorites, Recently played, Recently added, Mixes), Artists, Albums, Playlists (smart playlists with the rest) and Folders. More than 200 artists or albums come as one folder per first letter. Albums and an artist's albums show as a grid. A car with room for four tabs gets Home at the top of Playlists, under its own heading. With one library folder, Folders opens straight into it.
* Android Auto takes voice requests: "play Nirvana on PatinaPlayer" plays an artist or album named exactly, else the songs that match. Nothing said shuffles everything.
* The album grid builds only the tiles near the screen. With 8,500 albums it built all of them, menus and all, on every visit: opening Albums went from 1.9 s to 0.2 s in a debug build on an 85,000 song library, and opening it again from 1.6 s to 0.08 s.
* A new song no longer rebuilds every row of the open list, only moves the playing mark. The queue keeps each entry's row until its tags change, so a shuffled library in the queue doesn't rebuild 85,000 rows per song either.
* The last two big lists keep their built rows: going back to Songs went from 0.64 s to 0.14 s on 85,000 songs.
* A scan's or a sync's progress only redraws the sidebar's count. It used to rebuild the open view and the queue every time, which is what made the app crawl for the first minutes after opening while the library and the servers were checked.
* A sync, a scan or a song played to the end refreshes only the view on screen; the others refresh when you open them. A sync also waits longer between refreshes when reading the open view is slow, so it never takes more than about a tenth of the time. A song played to the end changes the play count in place in a list of more than 2,000, rather than reading all of it again.
* Settings opens at once: the sound cards and output devices are looked up on a thread of their own, and the rows fill in when the answer's in. Asking PipeWire took a round trip on the window's thread every time Settings opened.
* Covers load newest request first, and requests from rows scrolled past long ago are dropped, so the covers on screen when scrolling stops come first.
* The library database keeps 32 MB in memory instead of SQLite's 2 MB, with sorts in memory, and a folder's subfolders are told apart with a hash set rather than a list search.
* `cargo run --release -p patina-core --example bigbench` times every view's query on a made-up 85,000 song library.
* Tests: the cover search parsers, saving and replacing a chosen cover, a chosen cover winning over others, the backdrop settings, the car's tabs, letters and voice requests.

Reasons for changes: asked for a simpler phone Now Playing in the style of a screenshot, a cover backdrop over any theme, a way to find or upload missing covers, a heart and shuffle in Android Auto, buttons for artists, albums, playlists and folders there, Android Auto playback with the app closed, and a faster app on an 85,000 song library.

2026.11.13 - Feature addition (smart playlist leave-out box)

* Smart playlists have a second box, "Leave out songs where": any song its rules are true for is left out, whatever the first box says. Saved with the playlist, shown on its row as "… · leaving out …", and kept through renames and edits. In the terminal app it's the step after the first box.
* The live check says how many songs the second box left out ("Matches 1 track, 1 left out"), counted without the limit, so a leave-out rule that misses shows as 0.
* Mistakes in the second box start with "Leave out:", and sort or limit there is refused with a pointer to the first box.
* Fixed: `isnt` on a text field and `not ( … )` dropped songs that didn't have the tag at all, since SQL treats a missing tag as neither true nor false. A missing tag now counts as not matching, so `artist isnt "the beatles"` keeps songs with no artist. The leave-out box works the same way.
* Library database version 10 adds the second box's column. Existing smart playlists carry on with it empty.
* README: the second box, `=` for text and for year ranges (`year = 1960-1969`), and how songs without a tag are treated.
* Tests: the request's own rules (60s pop leaving out The Beatles written either way), a song without an artist kept on both sides, the left-out count with and without a limit, errors from the second box, saving, renaming, and SQL injection through the second box.

Reasons for changes: asked for a box of rules for songs to leave out, with year ranges usable alongside other rules. The tag fix came from making sure the second box couldn't lose untagged songs.

2026.11.13 - Feature addition (sleep timer)

* Sleep timer, from a moon button in the bottom bar and on Now Playing, or `Z` in the terminal app. Presets of 15, 30, 45, 60 and 90 minutes, the end of this song, or a time of your own: minutes, a length like 1h30, or a time of day like 23:30 or 11:30pm, with a live line saying when it will pause or what's wrong with what was typed.
* Over the last 30 seconds the volume comes down to -40 dB, then playback pauses and the volume is back for the next play. Bit-perfect output gets no fade, only the pause.
* "End of this song" pauses just before the song ends, or where a crossfade into the next would start, so none of the next song plays. It isn't offered for radio, which has no end.
* The timer runs in the player thread on the wall clock, so it keeps going with the window closed, the screen off, or the phone asleep. A time that passes while playback is already paused is dropped.
* Time left shows on Now Playing, at the top of the timer's menu and in the moon's tooltip, and in the terminal app's bottom line. "Sleep timer: paused" shows when it fires.
* In a very narrow desktop window, the volume slider is a little shorter so the moon and the percentage both fit.
* Tests for the timer: parsing lengths, times of day and refusals, the fade curve, pausing on time and playing again after, stopping at the end of the song before the next starts, and a time that passes while paused.
* Fixed a race in the Android Auto test, which checked the playing song before the player had marked it.

Reasons for changes: a sleep timer with a time of your own choosing was asked for. The test race turned up while running the full suite for it.

2026.11.12 - UX fixes (picked-rows bar, widget theme)

* The picked-rows bar takes every press that isn't on one of its buttons. A tap or long press on its "N picked" text, the gaps between buttons or its edge went through to the row underneath it.
* The home screen widget follows the app's theme: its ground, edge, text, buttons and play disc take the theme's colors, Material You included with the System theme. The app sends them whenever the theme changes, and the widget keeps them, so it stays in the theme after a restart. Before, it was always in Runout's colors.
* The widget's background and play disc are now white shapes tinted to the theme, since a widget can't recolor a view's background before Android 12 but can tint an image on every version.

Reasons for changes: long-pressing the picked-rows bar acted on the song underneath it, and the widget didn't match the app's theme.

2026.11.12 - UX overhaul (Now Playing)

* Now Playing has its own row at the top of the sidebar, lit while it's open. The cover in the bottom bar still opens and closes it.
* Now Playing closes when you go somewhere else: a sidebar row (even the view already underneath it), a search, Back or Forward, a breadcrumb, or Go to album or artist. Before, it stayed over the view you'd gone to until you closed it yourself.
* While Now Playing is open, no other sidebar row is lit, so the sidebar shows what's on screen.

Reasons for changes: Now Playing was only reachable from the small cover in the bottom bar, and it blocked the view you'd just gone to.

2026.11.12 - UX fixes (sizing)

* Dialogs grow to fit their help line. The smart playlist dialog's four-line help was measured as one line, so the dialog came out too short and cut off its Cancel and Create buttons. The help line now sits in the dialog's own layout, which measures wrapped text at its real width.
* Dialogs with fewer fields no longer leave an empty gap where the hidden fields were.
* Icon-only buttons are icon-sized again. Their tooltip's text was counted in their width, so on a narrow window "Add to queue" and "Add to playlist" came out as wide as their labels with only an icon inside.
* The picked-rows bar fits a phone: below 600 px wide its buttons show only their icons (Pick all gets a check mark, Play next becomes Next), with the full labels as tooltips. Before, most of them were off the right edge.
* Terminal app: text boxes longer than the box show their end, with a "…" in front, so the cursor stays in view. Before, a long export path, server address or smart playlist rule ran off the edge.

Reasons for changes: the smart playlist dialog was cut off at its bottom. Checking every wrapped text and narrow layout for the same kind of problem turned up the others.

2026.11.12 - UX overhaul (icon)

* New icon: a portable cassette player in cream, with an orange cassette and red trim on a dark brown tile. A simpler drawing of it is used at 16 to 32 px, where the full one's buttons and stripes blur.
* The AppImage has an icon again. linuxdeploy put the SVG at the top of the AppDir, appimagetool only makes `.DirIcon` from a PNG, and it built the AppImage anyway with a warning, so file managers, Gear Lever and AppImageLauncher had no icon to show. The build now puts the 256 px PNG on top and fails if `.DirIcon` is missing.
* The window tells the desktop its name (`patinaplayer`). On Plasma and GNOME under Wayland, the taskbar and window switcher find the icon through that name, and showed a generic one without it.
* The AppImage carries the icon at 16, 22, 24, 32, 48, 64, 128, 256 and 512 px plus the SVG.
* The tray icon uses the small drawing at 22 and 32 px, and its play/pause badge uses the icon's orange.
* Android gets an adaptive launcher icon, so it fills the launcher's shape instead of sitting shrunk on a white circle, and a one-colour layer for Android 13's themed icons.
* packaging/make-icons.sh renders every PNG from the SVGs with rsvg-convert.

Reasons for changes: the old icon didn't show on the AppImage or in the Wayland taskbar, and was asked to be replaced with a retro music player look. The cassette player was picked from six sketches, in the cassette sketch's colours.

2026.11.12 - Security patches

* Smart playlist rules have limits: 4000 characters, 200 conditions, 32 levels of parentheses or nots. Past them the rules get a plain message. Before, ten thousand nested parentheses could overflow the parser's stack and crash the app.
* A length like `99999999999999999:00` no longer overflows (a panic in debug builds). It's now reported as not a length.
* New test, smart_playlists_cannot_inject_sql: SQL injection payloads in every kind of value, SQL in place of fields, sort keys and the limit, a song whose tags are an attack, a saved playlist named as an attack, LIKE wildcards, nesting past the limits, and 4000 random rule strings that must either be turned away or run as valid SQL. It checks the library is untouched afterwards. Confirmed it fails if a value is ever pasted into the SQL or the LIKE escaping is dropped.
* README explains how the rules reach the database.
* The libgpod wrapper takes typed handles instead of raw pointers, so nothing outside it touches libgpod's memory directly.

Reasons for changes: smart playlist rules look like SQL, so they needed checking for injection. They were already safe, since every value was a bound parameter and every field, operator and sort order came from a fixed list. The check found the two crash bugs above, and the test now keeps it that way.

2026.11.12 - UX overhaul / Feature additions / Release workflow

* Pick several rows at once: Ctrl-click and Shift-click (or Shift+Up and Shift+Down) on the desktop, Space in the terminal app, long press then Select then tap on a phone. Play, queue, add to a playlist, keep offline, send to an iPod, or remove from a playlist or the queue, all on the picked rows. A bar at the bottom counts them.
* Drag to reorder the queue and playlists by a grip on each row. On touch screens, tap the grip to lift a song and tap where it goes.
* Now Playing view: big cover, song, album, artist, format, bit-perfect or not, the output, every stage of the sound chain touching the song, and lyrics beside it. Opens from the bottom bar's cover, or Ctrl+N in the terminal app.
* Tooltips on every icon-only button.
* Recently played view, grouped by day, with a Clear the history row. Albums and Songs sort by recently played.
* Smart playlists: saved rules that keep themselves up to date, checked live as you type, with sort and limit.
* M3U, M3U8 and PLS import, matching entries to the library by path, then by folder and file name. M3U8 export of playlists, smart playlists, mixes, the queue and any song list.
* ListenBrainz listens and Now Playing alongside Last.fm, with the token in the keyring, a custom server address, and the same per-server app/server/both choice.
* Android Auto browsing: Shuffle everything, Favorites, Playlists, Smart playlists, Recently played, Mixes and Recently added.
* Android home screen widget with the cover and back, play or pause, and next.
* iPod sync through libgpod: send songs, albums, playlists, smart playlists, mixes and Favorites, with conversion to Apple Lossless, AAC or MP3 by ffmpeg for what the iPod can't play. Take playlists and albums off, and eject.
* Terminal app: Space in a list now picks the row. `c` or Ctrl+Space pauses. New keys `U`, `H`, `S`, `E`, `<`, `>`, Ctrl+N and Ctrl+P.
* Release workflow now runs only from Run workflow on a tag that already has a release, fails in seconds with a clear message otherwise, attaches files with `gh release upload --clobber`, and adds a signed SLSA build provenance attestation to every APK. It no longer runs on a tag push.
* CI installs libgpod4 so the iPod test runs there.
* README: new features, keys, config sections, and a Making a release section. Added maintenance.md and changes.md.

Reasons for changes: the feature list signed off on 2026.11.12. Acting on many songs at once and reordering by hand were the biggest gaps in daily use. The Now Playing view, history and smart playlists cover what people expect from a library player. Import/export, ListenBrainz, the iPod and the Android Auto and widget work let the library reach the places music gets played. The release workflow was switched to the release-first, run-on-a-tag flow so a build never happens or ships by accident.
