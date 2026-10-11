# PatinaPlayer

PatinaPlayer is a Linux music player for Subsonic servers and local folders, for people who want a minimal interface on top of a serious playback chain.

This is milestone 1 of the design doc: the core audio engine and a terminal player that plays local files. Subsonic, the library views, EQ, leveling, bit-perfect, the amp stack and the GUI all come in later milestones. What's here is the part everything else sits on, so it's built to the final shape, not as a throwaway.

## What works right now

- Plays local files and folders passed on the command line. Folders are scanned recursively and sorted the way you'd expect (2 before 10).
- Anything FFmpeg decodes: FLAC, ALAC, WAV, AIFF, APE, WavPack, MP3, AAC, Opus, Vorbis and the rest.
- Gapless playback. Tracks are decoded back to back into one ring buffer, and FFmpeg trims MP3 and AAC encoder delay and padding, so live albums join without a click. A test proves the FLAC joins are sample-exact.
- Short fades on pause, resume, seek, next and stop (100 ms, adjustable, or off). No pops.
- PipeWire output, falling back to ALSA if no PipeWire daemon answers.
- Shuffle (the current track stays put), repeat off, all and one, and back restarts the track if you're more than 3 seconds in.
- Volume with a cubic curve, mute, and the format badge (FLAC 44.1/16, Opus 192) in the bottom bar.
- Mouse: click rows, double-click to play, click or drag the seek and volume bars, scroll the list.

One honest limitation: when the next track has a different sample rate (44.1 kHz into 48 kHz, say), the output reopens at the new rate and there's a short gap. Gapless across rates needs the resampler, which is milestone 10.

## Prerequisites

- Rust, latest stable. I use rustup, so in my case that's `rustup update stable`.
- FFmpeg 5.1 or newer, with dev headers
- PipeWire and ALSA dev headers
- clang and pkg-config, since the FFmpeg and PipeWire bindings are generated at build time

On Kubuntu (or any Ubuntu 24.04 based system):

```
sudo apt install pkg-config clang libavcodec-dev libavformat-dev libavutil-dev libswresample-dev libavfilter-dev libavdevice-dev libpipewire-0.3-dev libspa-0.2-dev libasound2-dev
```

On another distro, the package names are different but the list is the same: FFmpeg, PipeWire, libspa, ALSA, clang. Fedora calls them `ffmpeg-free-devel` (or `ffmpeg-devel` from RPM Fusion), `pipewire-devel` and `alsa-lib-devel`, for example. You'll have to check yours.

The tests also need the `ffmpeg` command itself, since they generate their own audio files.

## Building

1. Extract the archive and go into it: `cd patinaplayer`
2. Build it: `cargo build --release`
3. The binary is at `target/release/patinaplayer-tui`. Copy it somewhere on your PATH if you want it there, like `cp target/release/patinaplayer-tui ~/.local/bin/`, since that's where user binaries usually live.
4. Optionally run the tests: `cargo test`

## Usage

```
patinaplayer-tui ~/Music/Nirvana/Bleach
patinaplayer-tui song.flac other-song.opus ~/Music/SomeAlbum
patinaplayer-tui --shuffle ~/Music
```

Options:

| Option | What it does |
|---|---|
| `--backend auto\|pipewire\|alsa\|null` | Output. `auto` (the default) tries PipeWire, then ALSA. `null` plays to nothing, for testing on a box with no sound. |
| `--device NAME` | ALSA device (`default`, `hw:1,0`) or PipeWire node name. Leave it off for the system default. |
| `--buffer-ms N` | Output buffer, 50 to 1000. Default 200. |
| `--volume N` | Starting volume, 0 to 100. Default 80. |
| `--fade-ms N` | Fade length for pause, seek and so on. 0 turns fades off. Default 100. |
| `--shuffle` | Start shuffled. |
| `--log FILE` | Write a log. Logs never go to the terminal while the TUI is drawing, so this is the only way to see them. |

## Keys

| Key | Action |
|---|---|
| Space | Play / pause |
| n / b | Next / back |
| Left / Right | Seek 5 s |
| Shift+Left / Shift+Right | Seek 30 s |
| + / - | Volume |
| m | Mute |
| s | Shuffle |
| r | Cycle repeat (off, all, one) |
| Up / Down, j / k | Move the selection |
| PgUp / PgDn, g / G | Page up and down, top, bottom |
| Enter | Play the selection |
| . | Jump the selection back to the playing track |
| ? | Help |
| Ctrl+Q | Quit |

The rest of the doc's key map (views, search, EQ, queue panel, love) gets bound when those features exist.

## PipeWire and sample rates

PatinaPlayer asks PipeWire to run at the track's sample rate. Whether it does depends on your PipeWire config. Out of the box, most distros only allow 48 kHz, so a 44.1 kHz track gets resampled by PipeWire. To let it switch, add this to `~/.config/pipewire/pipewire.conf.d/rates.conf` (or whichever rates your DAC supports):

```
context.properties = {
    default.clock.allowed-rates = [ 44100 48000 88200 96000 176400 192000 ]
}
```

Then restart PipeWire with `systemctl --user restart pipewire`. That's still not bit-perfect, and the player never calls it that. Bit-perfect is the ALSA hw path in milestone 2.

## How it's put together

```
crates/
  patina-core/        shared core, used by the TUI now and the GUI later
    src/audio/
      decoder.rs      FFmpeg to interleaved f64, sample-accurate seek
      feeder.rs       decoder thread, gapless hand-off, ring management
      render.rs       real-time callback: fades, volume, DSP slots
      output/         PipeWire, ALSA and null backends
    src/player.rs     core thread: commands in, snapshots and events out
    src/queue.rs      queue, shuffle, repeat
    src/track.rs      metadata, format badge, folder scanning
    tests/gapless.rs  sample-exact join, MP3 padding, seek and rate change tests
  patina-tui/         the terminal frontend (ratatui)
```

The core owns all the state. Frontends only send commands and draw snapshots, which is what will keep the TUI and GUI identical later.

The audio thread never locks or allocates. Decoding happens on its own thread and feeds a lock-free ring, so a slow disk (or later, a slow server) stalls the ring, never the output. Seeks and track jumps fade the output down, flush the ring through an atomic handshake with the audio thread, then fade back in on the new audio.

All processing runs in 64-bit float. 16 and 24-bit PCM converts to f64 and back without losing a bit, which a test checks, since bit-perfect depends on it later.

## Known gaps in this milestone

- The TUI binary links FFmpeg, PipeWire and ALSA dynamically. The static TUI build in the doc is packaging work for milestone 14.
- Tags come from FFmpeg's metadata. Proper tag reading with lofty (ReplayGain fields included) comes with the library index in milestone 3.
- No config file yet. Everything is a command line flag.
- 16-bit ALSA output is rounded, not dithered. TPDF dither lands with the output controls in milestone 10. PipeWire output is f32, so this only matters on the ALSA fallback with a device that refuses float and 32-bit.

## License

GPL-3.0-or-later. See `LICENSE`.

Now PatinaPlayer is building, playing your files gaplessly through PipeWire, and the engine is ready for the bit-perfect path in milestone 2.
