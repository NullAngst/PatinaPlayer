# Changes

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
