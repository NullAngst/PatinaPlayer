# Changes

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
