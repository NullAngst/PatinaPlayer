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

Actions, Servers, Run workflow. It runs the clients against real Navidrome, Gonic, Ampache, MiniDLNA and Gerbera containers, pulled at their latest. A failure here usually means one of those servers changed its API or its container's setup. The log says which.

## 4. The release workflow

You don't need to release to check it. The parts that drift are the actions it uses:

- `actions/checkout`, `actions/cache`, `actions/upload-artifact`, `actions/download-artifact`, `actions/setup-java`, `actions/attest` and `Swatinem/rust-cache` are all on major-version tags, so they get fixes on their own. When GitHub announces a new major (a deprecation warning shows on the run's summary page), bump the number in `.github/workflows/*.yml`.
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
4. Remote control: turn it on in Settings on the desktop, then from a phone, Settings, Other devices, Control another player. The desktop should be listed by its machine name. `cargo test -p patina-core --test control` drives a real player through the client.
5. `mdns-sd` and `rust_cast` move fast. When updating either, run the two tests above and steps 1 and 4 by hand, since mDNS needs a real network.

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
