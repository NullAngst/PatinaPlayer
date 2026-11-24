# Maintenance

What to check every couple of months to keep PatinaPlayer building and working. Most of it is running things and looking at the result. Set aside an hour.

## 1. CI still goes green

1. Go to Actions and open the latest CI run on `main`.
2. If it's older than a month, click Run workflow on `main` to get a fresh one, since CI pulls the newest crates every run (`cargo update`) and a new release of something can break it without a single commit from you.
3. If Clippy fails on something new, that's usually a lint added in the latest Rust. Fix what it says. They're almost always one-line changes.
4. If the build fails in a dependency, look at its changelog for the version `cargo update` just pulled. Slint, `jni`, `rusqlite`, `reqwest` and `ffmpeg-next` are the ones that have broken things before with a new minor version.

## 2. Run the tests locally

1. `cargo update`
2. `cargo test --workspace`
3. `cargo clippy --workspace --all-targets -- -D warnings`

THE TESTS NEED A FEW GIGABYTES FREE FOR THE BUILD DIRECTORY. With debug info the test binaries add up fast; if you're short on space, `CARGO_PROFILE_DEV_DEBUG=0 cargo test --workspace` builds them a lot smaller.

## 2.5. Smart playlists stay injection-proof

`cargo test -p patina-core --test picking cannot_inject` throws SQL injection payloads and random rule strings at the smart playlist code. If you or anyone changes `crates/patina-core/src/library/smart.rs`, run it. The rule for that file: a value from the user only ever reaches SQL through `self.arg(...)`, which makes it a bound `?N` parameter. NEVER PUT A USER'S VALUE INTO THE SQL TEXT WITH `format!`. The only things `format!` may put into that SQL are the fixed strings in the code and numbers the code produced itself.

## 3. The Servers workflow

Actions, Servers, Run workflow. It runs the clients against real Navidrome, Gonic, Ampache, MiniDLNA and Gerbera containers, pulled at their latest. A failure here usually means one of those servers changed its API or its container's setup. The log says which. The container setup is what changes most: Gonic wanting a new folder mounted (it quits at start and the test then can't connect), or Ampache's image changing how it installs itself. For those, check `docker logs` in the job's Server logs step, then the image's own README (sentriz/gonic, ampache/ampache-docker) for what it wants now. Ampache installs from the `DB_*`, `MYSQL_*` and `AMPACHE_ADMIN_*` variables on its first start.

## 4. The release workflow

You don't need to release to check it. The parts that drift are the actions it uses:

- The builds themselves are in `.github/workflows/build.yml`, a reusable workflow. `appimage.yml` (Release) and `test-build.yml` (Test build) only call it, so a change to a build goes in `build.yml` once. Before a release, run Test build on the branch: it's the same builds with nothing published.
- `actions/checkout`, `actions/cache`, `actions/upload-artifact`, `actions/download-artifact`, `actions/setup-java`, `actions/attest`, `vmactions/freebsd-vm` and `Swatinem/rust-cache` are all on major-version tags, so they get fixes on their own. When GitHub announces a new major (a deprecation warning shows on the run's summary page), bump the number in `.github/workflows/*.yml`.
- The APK attestation needs `id-token: write` and `attestations: write` on the publish job. If GitHub changes what it asks for, the Attest the APK step says so.
- Check the release key secrets are still there (Settings, Secrets and variables, Actions): `ANDROID_KEYSTORE_BASE64`, `ANDROID_KEYSTORE_PASSWORD`, `ANDROID_KEY_ALIAS`, `ANDROID_KEY_PASSWORD`. KEEP A COPY OF THE KEYSTORE OUTSIDE GITHUB. Lose it and every phone has to uninstall to get the next version.

## 5. The Android side

The Java code is built without Gradle by `packaging/build-apk.sh`, against the newest SDK platform and NDK on the runner.

1. Install the latest APK from a release on your phone and open it.
2. Long press the home screen, add the PatinaPlayer widget, play something, and check the cover, title and buttons follow it.
3. In the car, or in the Android Auto desktop head unit (`desktop-head-unit` from the SDK's extras), browse each tab (Home, Artists, Albums, Playlists, Folders) and play something from each. Tap the heart and shuffle buttons beside the transport and check the app agrees, and that they show a heart and the shuffle arrows (not a blank square or circle). Albums, a letter and Recently added should show covers. If the buttons go blank after an Android Auto update, check the `ICON_` numbers in `PlaybackService.java` against `CommandButton.java` in androidx/media; if covers go missing, `adb logcat` while browsing shows any error from `ArtProvider`.
3.25. Play an album from the car, then open the car's queue screen (the list icon on Now Playing). It should list what's coming up with covers, the playing song marked, and tapping one should play it. The queue goes over in `Media::report_queue` (`crates/patina-android/src/lib.rs`) and `PlaybackService.queue`.
3.5. Close the app (swipe it away), then connect the car or start the head unit. It should browse and play without the app being opened. Then open the app on the phone: the same song keeps playing, with no restart. If the car says "Open PatinaPlayer on the phone" instead, the player couldn't start without the window; `adb logcat -s PatinaPlayer` says why.
4. The target SDK is whatever platform is newest on the runner (`TARGET_SDK` in `packaging/build-apk.sh`), so it moves up on its own. When it does, a new Android version's rules can change how background playback or notifications behave. If something stops working after a runner update, that's the first place to look.

## 5.5. Cover search and big libraries

1. Right-click an album, Find cover art. If one of iTunes, Deezer or MusicBrainz stops answering, its results just go missing: the parsers are `parse_itunes`, `parse_deezer` and `parse_musicbrainz` in `crates/patina-core/src/cover.rs`, each with a test of the answer's shape. Compare against a real answer (open the URL the code builds in a browser) and fix the field names there.
2. MusicBrainz asks for a User-Agent naming the app and a contact. It's in `http()` in the same file; keep the URL in it current.
3. Speed on a big library: `cargo run --release -p patina-core --example bigbench` makes a made-up 85,000 song library and times every view's query. Run it before and after anything that touches `library/mod.rs` or the queries in `browse/model.rs`. Nothing there should take more than a quarter of a second on a desktop.

## 5.6. Other devices and the remote control

1. Play on a Chromecast: cast button, pick it, and check the song comes out of it within a few seconds and that pause and skip follow. If it stops connecting after a Chromecast update, check `rust_cast` for a new version (it speaks Google's protocol) and the steps in `crates/patina-core/src/cast/chromecast.rs`.
2. Play on a DLNA renderer (gmrender-resurrect or Kodi will do, on any Linux box). `crates/patina-core/src/cast/dlna.rs` sends SetAVTransportURI, then Play. A renderer that refuses the stream usually says why in its SOAP fault; the app shows it.
3. Play on Snapcast: a snapserver with `source = tcp://0.0.0.0:4953?name=Patina&mode=server`. `cargo test -p patina-core --test cast` already checks the PCM that goes out, with a stand-in snapserver.
4. Remote control: turn it on in Settings on the desktop, then from a phone, Settings, Other devices, Control another player. The desktop should be listed by its machine name. `cargo test -p patina-core --test control` drives a real player through the client, and checks that New code and a restart on the same port work on the first try.
5. `mdns-sd` and `rust_cast` move fast. When updating either, run the two tests above and steps 1 and 4 by hand, since mDNS needs a real network.

## 5.7. Looks

1. Turn on the cover backdrop, Everywhere, and play an album with a near-white cover on a dark theme, then one with a near-black cover on Light. Muted text (the search placeholder, artist names in lists) should stay readable on both. The veil is `wash_veil` in `crates/patina-gui/src/bridge.rs`; its thresholds are the two numbers to nudge.
2. Click the cover on Now Playing through every visualizer style while a song plays, then pause: the bars should fall away. A Slint update that changes how `Path` takes its `commands` would show up here first. Then set `buffer_ms` to 500 (Settings, Output) and watch it again: it should move as smoothly as at 50. If it jumps a few times a second, the renderer's blocks aren't being spread out: check `Scope::clock` and `Analyzer::heard` (`crates/patina-core/src/audio/scope.rs`, `visual.rs`), and that `scope::LEN` still holds two of the biggest blocks.
3. Turn on Minimal UX and check the bottom bar and Now Playing have no badges, format or sound chain, and that the output button still opens the device list.

## 5.8. Smart playlist editor and Snapcast setup

1. Open each of the three rating playlists in the editor: they should open as rows, not text. Add a row of each kind (stars, a range, "in the last", a date, lossless) and save; reopening it should show the same rows. `cargo test -p patina-core form` covers the round trip. Then add a group with "…", put two rules in it, switch to text and back: the text should have the group in brackets and come back as the same rows. Open the field list on a phone: it should scroll to Source and stay clear of the navigation bar. That list is `Pick` in `app.slint`, a `PopupWindow` of our own; Slint's `Menu` doesn't scroll, so don't go back to it for long lists.
2. After a Snapcast update, check `crates/patina-core/src/cast/snapcast.rs` still matches its JSON-RPC: `Server.GetStatus` (streams under `server.streams`, each with `uri.scheme`, `uri.host`, `uri.query`) and `Stream.AddStream` with `streamUri`. The control API's doc is `doc/json_rpc_api/control.md` in the Snapcast repo. If Snapcast ever requires authentication for AddStream, the note will say it wouldn't add a source; the fix is then the line in `snapserver.conf`.

## 5.9. Plex, Icecast and Snapcast rooms

1. Plex's API docs live at developer.plex.tv/pms. Every couple of months, open its "API versioning" table. When a new version is listed, read what it changed against `crates/patina-core/src/remote/plex.rs`: the calls used are `/identity`, `/`, `/library/sections/all`, `/library/sections/{id}/all` (types 8 and 9), `/library/metadata/{id}/children`, `/library/metadata/{ids}`, `/library/metadata/{id}/nearest`, `/library/metadata/{id}/similar`, `/library/parts/...`, `/library/streams/{id}`, `/photo/:/transcode`, `/music/:/transcode/universal/start` (Opus in Matroska over HTTP) and `start.m3u8` (MP3 over HLS), `PUT /:/rate`, `POST /:/timeline` and `/playlists`. If nothing in them changed, raise `API_VERSION` in that file to the new version. The community OpenAPI spec (github.com/LukeHagar/plex-api-spec) is a quicker way to diff.
2. Check "Authenticating with Plex" on the same page still describes the PIN with a `jwk`, the `deviceJWT` poll, `/auth/nonce` and `/auth/token` returning `auth_token`. That flow is `crates/patina-core/src/remote/plex_tv.rs`. If Plex ever retires classic tokens, Add by hand stops working and the README should say so.
3. Watch the Plex forum thread "Question on https://clients.plex.tv/api/v2/resources and JWT authentication". If Plex answers it, the fallback to the account token in `Plex::connect` may be droppable, or may need replacing.
4. Sign in with a real Plex account, add a server, play a song through and check it shows on Plex's dashboard while it plays and its play count went up after. Rate a song and check Plex shows it.
5. Icecast: add any public Icecast 2.4+ server and check its mounts show. If Icecast changes `status-json.xsl`, `crates/patina-core/src/remote/icecast.rs` is where it's read.
6. Snapcast rooms use `Server.GetStatus`, `Group.SetClients`, `Group.SetStream` and `Client.SetVolume` from the control API doc in the Snapcast repo. Turn a room on that shares a group with another, check only it moves to the Patina stream, then turn it off and check it goes back. Then try it on a snapserver whose only stream is Patina's: turning a room off should add "Patina-idle" and move the room there, not fail.
7. Android: on a phone with Android 14 or later, open an album and swipe in from the left edge (it should go back to the artist), then at the top swipe from the left (the sidebar should open) and from the right (the app should go to the background, music playing).
8. Android, after a Slint update: start the app cold and look at the top and bottom. The header should start right under the status bar and the bottom bar end right over the navigation bar, with no extra band. The app reads the bars twice, from Slint (`safe-area-insets`) and from `MainActivity.reportInsets`, and uses the bigger; `sync_insets` in `bridge.rs` turns Android's pixels into window units at the window's current scale. If there's a band, check the scale isn't still 1.0 when the bars arrive.
9. Android, with Settings, Appearance, Cover backdrop set to everywhere: open Lyrics over a long list. No list rows should show through the panel, and the backdrop in it should line up with the one around it. That's `PanelGround` in `app.slint`. Pick a few songs: the list should run under the selection bar to the bottom, and the last song scroll up clear of it. Delete a playlist: the note should show its text with Undo at its right, go away on a tap, and go away when you open another page.

## 5.10. The Drive screen, folded Settings and the library backup

1. With Android Auto connected (or car mode on through a driving mode), check the Drive screen shows, swiping skips, the buttons work, Full app leaves it and the Drive button brings it back, and that it goes away when car mode ends. Rotate the phone to check the landscape layout. The screen is `DriveView` in `crates/patina-gui/ui/app.slint`.
2. Open Settings: every group should start folded. If a new group was added, it folds like the rest without anything to change; `cargo test -p patina-core --test browse settings_groups` checks it. Open one: its rows should be indented and sit on one panel in the theme's `sel-idle` color, rounded under the last row, with hover and selection showing on it. Check it in Runout and Light, where the colors are closest. The panel is drawn in `ListRow` in `app.slint`; which rows get it is `Row.nest`, set in `bridge.rs`.
4. On Android, Settings should have no Desktop, iPod or Files group, and no Sample format, Dither or Hold on metered connections. The list is `DESKTOP_GROUPS` and `DESKTOP_KEYS` in `crates/patina-core/src/browse/settings.rs`; a new setting that does nothing on a phone goes there. `cargo test -p patina-core --lib android_leaves_out` checks it. If Android metered detection is ever added (ConnectivityManager), take `PauseMetered` off the list.
3. Back up the library on the phone to Downloads, and on the desktop. Open the copy with `sqlite3 copy.db "PRAGMA integrity_check"`; it should say ok. Android's save dialog is `Picker.save` in `android/java/dev/patina/player/Picker.java`.

## 5.11. Source scope, Opus from Plex and staying connected

1. Pick a server in the sidebar's source picker and check Artists, Albums, Songs, Genres, Folders and search show only its music, then Local, then All sources. With a local album and the same album on Plex, All sources should show it once, from whichever is first in Settings, Library, Source order. `cargo test -p patina-core --lib library::scope` and `cargo test -p patina-core --test browse source_scope` check this. The filter is two temporary views, `s_tracks` and `s_albums`, in `crates/patina-core/src/library/scope.rs`. A new browsing query in `library/mod.rs` should read `self.tracks_t()` and `self.albums_t()`, not the plain tables, or it ignores the picker.
2. Android Auto: with a server picked on the phone, the car's lists should show only that server.
3. Set a Plex server to Opus 128 and play a song. If it won't play, the note now gives the download's error. Plex picks a client profile by X-Plex-Platform; if Plex adds or renames profiles and conversions stop, that's where to look. Plex has said it never picks MP3 by itself, so the MP3 choice relies on `X-Plex-Client-Profile-Extra`.
4. Plex songs as the original file are looked up with `/library/metadata/{id}?checkFiles=1` before each download (`Plex::part_key`). If Plex drops the `exists` and `accessible` attributes, nothing breaks, the app just stops naming files Plex can't open. A song that fails with HTTP 500 and no such note: check the Plex Media Server log on the server for "Error opening file".
5. A conversion Plex fails on (5xx before any data) is asked for again once, then downloaded as the original (`download_song` in `crates/patina-core/src/cache/service.rs`). The log line "couldn't convert song" carries Plex's reason from `/music/:/transcode/universal/decision` (`Plex::stream_trouble`). If those show up for many songs, the reasons say whether it's Plex's profiles (look at `transcode_query` in `plex.rs`) or the files. `cargo test -p patina-core --test cache conversion` checks the retry and the fallback against the stand-in server, and `cargo test -p patina-core --test cache an_original` the other way round (an original Plex fails on comes converted).
6. Plex's HTTP 500 and 501 are `RemoteError::Server` (Plex answered, and failed), and 502 to 504 stay `RemoteError::Http` (a proxy whose Plex isn't answering). Only `Http` takes a server offline. That mapping is in `send` in `plex.rs`; `cargo test -p patina-core --test plex failing_at_a_request` checks it. If a reverse proxy in front of Plex ever answers 500 for "backend down", it would need adding to the `Http` side.
7. `cargo test -p patina-core --test player away` checks that three songs in a row on a server that isn't connected stop playback with one message (`MAX_AWAY_IN_A_ROW` in `audio/feeder.rs`).
5. Leave the app open on a phone for ten minutes, moving between Wi-Fi and mobile data. The Plex page shouldn't show it offline. If it does, the reason it shows is the place to start; the ping is in `RemoteService::ping`, the retry in `Plex::call`, and the idle timeout in `remote/http.rs`.

## 5.12. Slint's Android mouse patch

Slint's Android backend panics on a mouse wheel (`MotionAction::Scroll => todo!()` in `androidwindowadapter.rs` of the `i-slint-backend-android-activity` crate) and takes a mouse click as a finger press on top of the button press. `packaging/patch-slint-android.sh` patches it for every APK build with `packaging/slint-android-mouse.patch`, at whatever version `Cargo.lock` has. Nothing to do while the build log says "Patched i-slint-backend-android-activity <version> for mice".

1. When a Slint release comes out, open its changelog and the crate's `androidwindowadapter.rs` (on docs.rs, "Source", or in `~/.cargo/registry/src/` after `cargo fetch --target aarch64-linux-android`). Search for `MotionAction::Scroll`.
2. Still `todo!()`, and the build says the patch no longer applies: redo it. Download the new version (`curl -LO https://static.crates.io/crates/i-slint-backend-android-activity/i-slint-backend-android-activity-<version>.crate`, then `tar xzf` it), copy `androidwindowadapter.rs` to `a/` and `b/`, make the same two changes in `b/` (the `mouse` arms at the top of `match motion_event.action()`, and the `Scroll` arm), and write the patch with `diff -U2 a/androidwindowadapter.rs b/androidwindowadapter.rs > packaging/slint-android-mouse.patch`. Then take the dates off the first two lines of the patch.
3. Slint handles scroll itself now (the build warns that it's going on without the patch): install that APK on a phone with a mouse, scroll a list with the wheel and right-click a song. If both work, delete `packaging/slint-android-mouse.patch` and `packaging/patch-slint-android.sh`, and the block that calls it in `packaging/build-apk.sh`. If right-click presses a row as well as opening its menu, keep the `mouse` part of the patch and drop the `Scroll` hunk.
4. Either way, check a mouse on a phone after a Slint update: wheel scrolling in a list and in Settings, right-click on a song (menu, nothing else), left click plays, a drag with the button held scrolls the list.

## 5.13. Snapcast listening, Info, artist pictures, smart playlist files

1. Snapcast listening is `crates/patina-core/src/snapclient.rs`: Snapcast's binary protocol (`doc/binary_protocol.md` in the Snapcast repo). `cargo test -p patina-core --lib snapclient` checks it against a stand-in snapserver. After a Snapcast release, check that doc for a new `SnapStreamProtocolVersion` or changed message layouts (the 26-byte base header, Hello, Codec Header, Wire Chunk, Time). On a real snapserver, Listen here on each codec it offers (`codec = flac`, `ogg`, `opus`, `pcm` in `snapserver.conf`) and check it plays and that the stream named is the one heard. Picking the stream is `listen_route` in `cast/snapcast.rs` (`Group.SetClients`, `Group.SetStream`).
2. If a listener is ever left in Snapweb's client list on its own, that's just the snapserver remembering it; delete it there. Its id is in `snapclient-id` in the app's data folder.
3. Find artist picture asks Deezer's `api.deezer.com/search/artist`. If pictures stop coming, open that URL with `?q=Nirvana` in a browser and compare with `artist_art::parse_deezer`. Deezer's placeholder for an artist without a picture has `/artist//` in its address, and is left out.
4. Info on a song, an album and Now Playing: check the file's own tags section shows for a local FLAC and an MP3. Tag names come from lofty's `ItemKey`; a lofty update that renames them changes the labels only.
5. Export a smart playlist's rules on one device and import the file on another. The format is `crates/patina-core/src/library/smart_io.rs`; `cargo test -p patina-core --lib smart_io` round-trips it. If the smart playlist language ever changes in a way old rules don't read, the import lists them as left out rather than failing the rest.
6. On Android, Import in Playlists should open the system's file picker and Export (a playlist, the queue, a smart playlist's rules) its save dialog. That's `Picker.start` with `PICK_DOCUMENT` and `Picker.save` in `android/java/dev/patina/player/Picker.java`. The save dialog's file type comes from the extension through Android's `MimeTypeMap`, so the name is kept as given; if a provider ever adds `.m3u` after `.m3u8`, that's where to look.

## 5.14. The touch sheet, panel tabs, undo and the palette

1. The touch sheet's rows are built in Rust (`row_sheet` in `crates/patina-gui/src/bridge.rs`), and the desktop menu is in Slint (the `ContextMenuArea` blocks in `ListRow`, `AlbumTile` and `QueueLine` in `ui/app.slint`). They have to offer the same things. When a menu item is added or changed in one, do it in the other, with the same action name, since both end up in the same `act` handler. Quick check: long press a song, an album, an artist, a folder, a playlist, a smart playlist, a station and a queue row with a finger, then right-click the same with a mouse, and compare.
2. Touch or mouse comes from `Ui.saw(e)` at the top of every `pointer-event` handler in `app.slint`, which reads `touch-finger-id` (0 is a mouse). A new `TouchArea` with a `pointer-event` handler needs that line too. After a Slint update, check `touch-finger-id` is still in `PointerEvent` (`cargo check -p patina-gui` fails if it's gone) and that a mouse on Android still reads as a mouse.
3. Undo lives in `crates/patina-core/src/browse/undo.rs`. Something new that deletes or takes things out should call `offer_undo` with a way back, not add a confirm. Anything that can't be put back (a server deleting for good, removing a server, clearing the cache) keeps its confirm. `cargo test -p patina-core --test browse --test picking` covers a station, a smart playlist, a playlist and songs taken out of one.
4. The terminal app's palette is `crates/patina-tui/src/palette.rs`. A new key binding should get a line there too, or the palette falls behind the keys. `cargo test -p patina-tui palette` checks the matching.
5. Settings search matches the label, the help line and the group name (`model.rs`, the `filter` words). A setting nobody can find usually needs a better help line.
6. Sidebar folding: with a fresh config and a library folder set, Sources and Play on should start folded; click them open, restart, and they should stay open.

## 5.15. Phone controls, lists and the scrubber

1. Android volume: with a song playing, press the phone's volume buttons. The slider on Now Playing should follow within a quarter of a second, and moving the slider should move the phone's own volume panel's level. The app's own volume should stay at 100% (`volume` in the session). It's `system_volume` and `set_volume` in `bridge.rs`, and `musicVolume`, `setMusicVolume` in `Native.java`. Casting and bit-perfect keep the app's own volume, on purpose.
2. Haptics: long press a row, pick rows, drag on the scrubber. Each should buzz lightly, and nothing at all with Android's touch feedback turned off (the buzz goes through `View.performHapticFeedback`, which follows that setting). If a newer Android drops one of the `HapticFeedbackConstants`, change it in `Native.haptic`.
3. Bluetooth: play a song, connect and disconnect a Bluetooth headset. The song should pause where it was each time, and Play should carry on from there. That's `output_lost` in `crates/patina-core/src/player.rs`; `cargo test -p patina-core --test player a_lost_output` checks it with a failing output.
4. The letter scrubber shows on Artists, Albums and Songs with more than 40 rows, sorted A to Z. Sort Albums by year: it should go away. It reads the column the list is sorted by from the rows themselves (`scrub_index` in `bridge.rs`), assuming SQLite's NOCASE order: digits and marks first, then a to z, then everything past ASCII. If the library ever sorts by another collation (one that ignores "The", say), `scrub_bucket` has to follow it.
5. Glyphs: Android has no fallback fonts for Slint, so a character none of the bundled fonts has comes out as an empty box there, though the desktop finds it in a system font. After adding one to `app.slint`, check it's in the fonts the text uses (`pip install fonttools`, then `TTFont(path).getBestCmap()` has its code point), or draw it as an icon path, as the row glyphs for Hearted songs and smart playlists are now.
5.5. Fast scrolling: on a phone, flick a long list quickly, several short flicks in a row, from the middle and from the top. Each flick should move the list. Nothing may wrap the list in a SwipeGestureHandler (`app.slint`): an enabled one holds every touch back for about 100 ms to decide whether it is a swipe, and a flick shorter than that never reaches the list. That is why there is no pull to refresh. If a Slint release adds overscroll to Flickable, pull to refresh can come back on top of that.
6. Going back to a view should land where it was scrolled to (`scroll_mem` in `bridge.rs`, keyed by the level). After a Slint update, check the ListView still takes `content-y` set from code when its rows have just changed.
7. Synced lyrics, centered: the playing line should sit in the middle of the panel and the view slide smoothly from line to line. The panel finds the line by its `y` (`lyr-mid` in `app.slint`); if a Slint update stops `changed y` firing for a line in a layout, the lyrics stop scrolling.

## 5.16. The FreeBSD build

1. On each release, check the FreeBSD job's log. Its prepare and run steps print each command before running it and stop at the first that fails, so when the job is red, open "Run 'prepare' in VM" (then "Run 'run' in VM") and the last `+` line above the error is the one that failed. The first lines of prepare give the FreeBSD version, the VM's free disk and its memory; a build that dies partway with no clear error is usually one of those running out. `vmactions/freebsd-vm@v1` runs whichever FreeBSD release it defaults to, so a new FreeBSD comes in by itself. If a package was renamed (`pkg install` fails), fix its name in `.github/workflows/appimage.yml`, in `packaging/build-freebsd.sh` and in the README.
2. The `pipewire-sys` patch: the log says "Patched pipewire-sys <version> for FreeBSD". If it says the patch no longer applies, redo it: download the crate (`curl -LO https://static.crates.io/crates/pipewire-sys/pipewire-sys-<version>.crate`, then `tar xzf` it), copy `build.rs` and `src/lib.rs` to `a/` and `b/`, make the same two changes in `b/` (allowlist `pipewire_init` and `pipewire_deinit` in `build.rs`, and the `#[cfg(target_os = "freebsd")] pub use` aliasing them to `pw_init` and `pw_deinit` in `lib.rs`), and write the patch with `diff -U2`, both files into `packaging/pipewire-sys-freebsd.patch`, with the dates taken off the `---` and `+++` lines.
3. If the log says pipewire-sys handles FreeBSD itself, the fix went upstream: delete `packaging/pipewire-sys-freebsd.patch`, `packaging/patch-pipewire-freebsd.sh` and the block in `build-freebsd.sh` that calls it, and the paragraph about it in the README. The same goes if FreeBSD's PipeWire port stops renaming `pw_init` (check `multimedia/pipewire/files/` in the ports tree for the `patch-src_pipewire_pipewire_init` file).
4. Every couple of months, see whether anything else in the build has gone Linux-only: `cargo check` in the FreeBSD job fails first. The usual suspects are new dependencies with `/proc` or `inotify` assumptions.

## 6. ListenBrainz and Last.fm

1. In Settings, check the ListenBrainz row still shows your user name. If it says the token is refused, make a new one at listenbrainz.org/settings and paste it in.
2. Play a song past half way, then look at your ListenBrainz and Last.fm profiles for it.
3. ListenBrainz's API docs are at listenbrainz.readthedocs.io. The app uses three calls: `validate-token`, and `submit-listens` with `single`, `import` and `playing_now`. If those get a new version, `crates/patina-core/src/listenbrainz.rs` is the only file that talks to them.

## 7. libgpod and the iPod sync

libgpod hasn't had a release since 0.8.3 in 2013, so its structs don't move. The app reads a few fields at fixed offsets (`crates/patina-core/src/ipod/gpod.rs`). If a distro ever ships a new libgpod, check them:

1. Install the headers: `sudo apt install libgpod-dev`, or whichever name your distro uses.
2. Save this as `offsets.c`:

```c
#include <stdio.h>
#include <stddef.h>
#include <gpod/itdb.h>
#define T(f) printf("%-12s %zu\n", #f, offsetof(Itdb_Track, f))
int main(void) {
    T(title); T(ipod_path); T(album); T(artist); T(genre); T(filetype);
    T(composer); T(albumartist); T(size); T(tracklen); T(cd_nr); T(cds);
    T(track_nr); T(tracks); T(bitrate); T(samplerate); T(year);
    T(time_added); T(mediatype);
    printf("%-12s %zu\n", "db.tracks", offsetof(Itdb_iTunesDB, tracks));
    printf("%-12s %zu\n", "db.playlists", offsetof(Itdb_iTunesDB, playlists));
    printf("%-12s %zu\n", "pl.name", offsetof(Itdb_Playlist, name));
    printf("%-12s %zu\n", "pl.members", offsetof(Itdb_Playlist, members));
    return 0;
}
```

3. Build and run it: `cc offsets.c $(pkg-config --cflags libgpod-1.0) -o offsets && ./offsets`
4. Compare the numbers with the `track` and `db` tables at the top of `gpod.rs`. They should be identical. If one differs, change it there and run `cargo test -p patina-core --test picking`.

The iPod test in `tests/picking.rs` builds a fake iPod in a folder, sends songs to it (one converted from FLAC), and reads them back. CI installs `libgpod4` so it runs there. Without libgpod it skips itself and passes, so check the log says it ran.

With a real iPod, every few months is enough: plug it in, send an album, eject, and play it on the iPod.

## 7.5. The icon

The SVGs in `assets/` are the source of every icon; the PNGs are rendered from them. After changing an SVG, run `bash packaging/make-icons.sh` and commit the PNGs it writes. It needs `rsvg-convert`.

Once in a while, after a release:

1. Check the AppImage still has its icon: `./PatinaPlayer-*.AppImage --appimage-extract .DirIcon` and open `squashfs-root/.DirIcon`. It should be the 256 px PNG. The build fails if it's missing, but a linuxdeploy update could change what it puts there.
2. Start the AppImage and look at the taskbar and the Alt+Tab switcher. A generic icon there means the window's name no longer matches the desktop file: check `slint::set_xdg_app_id` in `crates/patina-gui/src/main.rs` is still called before the window is shown, and that it matches `StartupWMClass` in `assets/patinaplayer.desktop`.
3. On the phone, the launcher icon should fill the launcher's shape, and turn one colour with themed icons on.

## 8. Things that age on their own

- The bundled fonts in `crates/patina-gui/ui/fonts/`. Check Inter and JetBrains Mono for new releases once a year. They're drop-in replacements.
- The AppImage's minimum glibc follows the runner. When `ubuntu-24.04` is retired, move both workflows to the next LTS and update the glibc line in the README.
- `changes.md`: add an entry for every update, in the same format as the ones there.
