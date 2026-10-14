//! FFmpeg decoding to interleaved f64 at the source sample rate.
//!
//! Sample conversion is done by hand rather than through swresample, since
//! a plain integer-to-float scale is exact and reversible. That matters once
//! the bit-perfect path lands: 16 and 24-bit PCM survive the round trip
//! through f64 untouched.
//!
//! Encoder delay and padding (MP3 LAME tags, AAC priming, Opus pre-skip)
//! are trimmed by libavcodec itself through skip-samples side data, so
//! gapless joins come out sample-exact without extra work here.

use std::path::Path;
use std::sync::Arc;
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use ffmpeg::format::sample::{Sample, Type as SampleType};
use ffmpeg_next as ffmpeg;

use super::{Depth, StreamFormat};
use crate::track::TrackInfo;

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("{0}")]
    Ffmpeg(#[from] ffmpeg::Error),
    #[error("no audio stream in file")]
    NoAudio,
    #[error("unsupported sample format {0:?}")]
    Format(Sample),
    #[error("unsupported channel count {0}")]
    Channels(u16),
}

static INIT: Once = Once::new();

pub fn init_ffmpeg() {
    INIT.call_once(|| {
        let _ = ffmpeg::init();
        // FFmpeg logs straight to stderr, which would scribble over the
        // TUI. Quiet unless asked for; dropped streams and bad packets are
        // reported through our own errors anyway.
        let level = if std::env::var_os("PATINA_FFMPEG_LOG").is_some() {
            ffmpeg::util::log::Level::Warning
        } else {
            ffmpeg::util::log::Level::Quiet
        };
        ffmpeg::util::log::set_level(level);
    });
}

/// How to open something. The defaults are right for a local file.
#[derive(Debug, Clone, Default)]
pub struct OpenOptions {
    /// A live stream: ask for ICY metadata, never treat end of stream as
    /// end of track.
    pub live: bool,
    /// Podcast playback speed with pitch kept. 1.0 (or 0) is off.
    pub tempo: f64,
    /// Cut long silences out (podcasts).
    pub trim_silence: bool,
    /// FFmpeg checks this while it waits on the network. Setting it makes a
    /// stalled read give up at once instead of after the timeout.
    pub abort: Option<Arc<AtomicBool>>,
}

impl OpenOptions {
    fn filtered(&self) -> bool {
        (self.tempo > 0.0 && (self.tempo - 1.0).abs() > 1e-3) || self.trim_silence
    }
}

pub fn is_network(loc: &str) -> bool {
    let l = loc.to_ascii_lowercase();
    [
        "http://", "https://", "icy://", "rtmp://", "rtsp://", "mms://", "mmsh://",
    ]
    .iter()
    .any(|p| l.starts_with(p))
}

/// A libavfilter chain for tempo and silence trimming. Output is packed
/// f64, so it drops straight into the ring like decoded audio does.
struct Filter {
    graph: ffmpeg::filter::Graph,
    out: ffmpeg::frame::Audio,
    /// What comes out, fixed for the life of the source.
    out_format: StreamFormat,
    /// Source time at the end of the last frame fed in.
    src_end: f64,
    flushed: bool,
}

pub struct Source {
    input: ffmpeg::format::context::Input,
    decoder: ffmpeg::decoder::Audio,
    stream_index: usize,
    time_base: f64,
    format: StreamFormat,
    info: TrackInfo,
    frame: ffmpeg::frame::Audio,
    packet: ffmpeg::Packet,
    eof_sent: bool,
    /// The decoder has handed over its last frame (filtered path).
    eof_drained: bool,
    done: bool,
    /// Frames still to drop after a seek, so the seek lands on the exact
    /// sample rather than wherever the demuxer's nearest packet was.
    skip_until: Option<f64>,
    opts: OpenOptions,
    filter: Option<Filter>,
    /// Last title seen in ICY or timed metadata.
    last_title: Option<String>,
    /// `frame` holds a decoded frame not handed out yet: the one decoded at
    /// open to learn the real format.
    primed: bool,
    /// The stream switched sample rate or channels mid-play. The samples
    /// returned by the last decode are already in the new format.
    format_changed: bool,
    depth: Depth,
}

/// Highest channel count we pass through. FFmpeg can report more, but
/// nothing downstream knows what to do with a 24-channel file.
pub const MAX_CHANNELS: u16 = 8;

impl Source {
    pub fn open(path: &Path) -> Result<Self, DecodeError> {
        Self::open_with(&path.to_string_lossy(), OpenOptions::default())
    }

    /// Open a file path or a URL. Network sources get FFmpeg's reconnect,
    /// a read timeout and, for live streams, ICY metadata.
    pub fn open_with(loc: &str, opts: OpenOptions) -> Result<Self, DecodeError> {
        init_ffmpeg();
        let input = if is_network(loc) {
            let mut d = ffmpeg::Dictionary::new();
            d.set(
                "user_agent",
                concat!("PatinaPlayer/", env!("CARGO_PKG_VERSION")),
            );
            // Read stall timeout, in microseconds.
            d.set("rw_timeout", "15000000");
            d.set("reconnect", "1");
            d.set("reconnect_on_network_error", "1");
            d.set("reconnect_delay_max", "10");
            if opts.live {
                d.set("icy", "1");
                d.set("reconnect_streamed", "1");
            }
            let abort = opts.abort.clone();
            ffmpeg::format::input_with_interrupt_and_dictionary(
                loc,
                move || abort.as_ref().is_some_and(|a| a.load(Ordering::Relaxed)),
                d,
            )?
        } else {
            ffmpeg::format::input(loc)?
        };
        let path = if is_network(loc) {
            Path::new("")
        } else {
            Path::new(loc)
        };
        let stream = input
            .streams()
            .best(ffmpeg::media::Type::Audio)
            .ok_or(DecodeError::NoAudio)?;
        let stream_index = stream.index();
        let tb = stream.time_base();
        let time_base = tb.numerator() as f64 / tb.denominator().max(1) as f64;
        let ctx = ffmpeg::codec::context::Context::from_parameters(stream.parameters())?;
        let decoder = ctx.decoder().audio()?;

        let channels = decoder.channels();
        let format = StreamFormat {
            rate: decoder.rate(),
            channels,
            mask: channel_mask(&decoder, channels),
        };
        let mut info = TrackInfo::from_input(path, &input, stream_index);
        if opts.live {
            info.duration = None;
        }
        let depth = match info.bits {
            Some(b) if info.lossless && is_int(decoder.format()) => Depth::Int(b.min(32) as u8),
            _ => Depth::Float,
        };

        let mut src = Self {
            input,
            decoder,
            stream_index,
            time_base,
            format,
            info,
            frame: ffmpeg::frame::Audio::empty(),
            packet: ffmpeg::Packet::empty(),
            eof_sent: false,
            eof_drained: false,
            done: false,
            skip_until: None,
            opts,
            filter: None,
            last_title: None,
            primed: false,
            format_changed: false,
            depth,
        };
        // Decode the first frame now. What the container header says isn't
        // always what comes out: HE-AAC streams announce half their sample
        // rate (and often mono) and decode to the full rate in stereo. The
        // frame is the truth, and it's kept to be handed out first.
        src.prime()?;
        if src.format.channels == 0 || src.format.channels > MAX_CHANNELS {
            return Err(DecodeError::Channels(src.format.channels));
        }
        if src.format.rate == 0 {
            return Err(DecodeError::NoAudio);
        }
        if src.opts.filtered() {
            src.filter = Some(Filter::new(
                src.decoder.format(),
                src.format,
                src.format,
                tb,
                &src.opts,
            )?);
        }
        if src.opts.live {
            src.apply_icy_headers();
        }
        Ok(src)
    }

    pub fn is_live(&self) -> bool {
        self.opts.live
    }

    /// Sample depth of the source, for the bit-perfect output.
    pub fn depth(&self) -> Depth {
        self.depth
    }

    /// True once after the stream changed format mid-play. The samples the
    /// last [`decode_into`](Self::decode_into) returned are in the new
    /// [`format`](Self::format).
    pub fn take_format_change(&mut self) -> bool {
        std::mem::take(&mut self.format_changed)
    }

    /// Decode until the first frame with samples, and take the stream's
    /// format from it. An empty stream keeps the header's format.
    fn prime(&mut self) -> Result<(), DecodeError> {
        let n = self.decode_raw_frame()?;
        if n > 0 {
            self.primed = true;
            self.format = frame_format(&self.frame);
            self.info.sample_rate = self.format.rate;
            self.info.channels = self.format.channels;
        }
        Ok(())
    }

    /// For filtered sources, where in the source the audio returned so far
    /// ends. Tempo and silence trimming break the plain frames-to-seconds
    /// mapping, so the feeder uses this to keep the position honest.
    pub fn source_position(&self) -> Option<Duration> {
        self.filter
            .as_ref()
            .map(|f| Duration::from_secs_f64(f.src_end.max(0.0)))
    }

    /// Station name and bitrate from the ICY response headers.
    fn apply_icy_headers(&mut self) {
        let Some(h) = self.read_opt("icy_metadata_headers") else {
            return;
        };
        for line in h.lines() {
            let Some((k, v)) = line.split_once(':') else {
                continue;
            };
            let v = v.trim();
            if v.is_empty() {
                continue;
            }
            match k.trim().to_ascii_lowercase().as_str() {
                "icy-name" => self.info.album = Some(v.to_string()),
                "icy-br" => {
                    if let Some(br) = v.split(',').next().and_then(|b| b.trim().parse().ok()) {
                        self.info.kbps = Some(br);
                    }
                }
                _ => {}
            }
        }
    }

    /// Read a string AVOption from the format context or its children
    /// (the http protocol is where the ICY fields live).
    fn read_opt(&mut self, name: &str) -> Option<String> {
        let cname = std::ffi::CString::new(name).ok()?;
        // SAFETY: av_opt_get allocates the out string, which we free.
        // Both the format context and its pb are valid while self lives.
        unsafe {
            let ctx = self.input.as_mut_ptr();
            let try_obj = |obj: *mut std::ffi::c_void| -> Option<String> {
                if obj.is_null() {
                    return None;
                }
                let mut out: *mut u8 = std::ptr::null_mut();
                let r = ffmpeg::ffi::av_opt_get(
                    obj,
                    cname.as_ptr(),
                    ffmpeg::ffi::AV_OPT_SEARCH_CHILDREN,
                    &mut out,
                );
                if r < 0 || out.is_null() {
                    return None;
                }
                let s = std::ffi::CStr::from_ptr(out as *const std::ffi::c_char)
                    .to_string_lossy()
                    .into_owned();
                ffmpeg::ffi::av_free(out as *mut std::ffi::c_void);
                (!s.is_empty()).then_some(s)
            };
            try_obj(ctx as *mut _).or_else(|| try_obj((*ctx).pb as *mut _))
        }
    }

    /// A new now-playing title, if the stream sent one since last asked.
    /// ICY metadata for Icecast and SHOUTcast, timed ID3 for HLS.
    pub fn poll_title(&mut self) -> Option<String> {
        let title = self
            .read_opt("icy_metadata_packet")
            .and_then(|p| parse_stream_title(&p))
            .or_else(|| self.timed_metadata_title());
        match title {
            Some(t) if self.last_title.as_deref() != Some(t.as_str()) => {
                self.last_title = Some(t.clone());
                Some(t)
            }
            _ => None,
        }
    }

    fn timed_metadata_title(&self) -> Option<String> {
        let stream = self.input.stream(self.stream_index)?;
        let get = |d: ffmpeg::DictionaryRef, keys: &[&str]| {
            d.iter()
                .find(|(k, _)| keys.iter().any(|x| k.eq_ignore_ascii_case(x)))
                .map(|(_, v)| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let title = get(stream.metadata(), &["StreamTitle", "title", "TIT2"])
            .or_else(|| get(self.input.metadata(), &["StreamTitle"]))?;
        match get(stream.metadata(), &["artist", "TPE1"]) {
            Some(a) => Some(format!("{a} - {title}")),
            None => Some(title),
        }
    }

    /// Format of the samples [`decode_into`](Self::decode_into) returns.
    pub fn format(&self) -> StreamFormat {
        match &self.filter {
            Some(f) => f.out_format,
            None => self.format,
        }
    }

    pub fn info(&self) -> &TrackInfo {
        &self.info
    }

    /// Seek to `pos`. The next samples returned start exactly there.
    pub fn seek(&mut self, pos: Duration) -> Result<(), DecodeError> {
        let ts = (pos.as_secs_f64() * ffmpeg::ffi::AV_TIME_BASE as f64) as i64;
        // Seek to a keyframe at or before the target, then drop samples
        // until we reach it.
        if self.opts.live {
            return Ok(());
        }
        self.input.seek(ts, ..ts)?;
        self.decoder.flush();
        self.primed = false;
        self.eof_sent = false;
        self.eof_drained = false;
        self.done = false;
        self.skip_until = Some(pos.as_secs_f64());
        if self.filter.is_some() {
            // Tempo and silence filters carry state across samples; a seek
            // starts them fresh.
            let tb = self.input.stream(self.stream_index).unwrap().time_base();
            let out = self
                .filter
                .as_ref()
                .map(|f| f.out_format)
                .unwrap_or(self.format);
            let mut f = Filter::new(self.decoder.format(), self.format, out, tb, &self.opts)?;
            f.src_end = pos.as_secs_f64();
            self.filter = Some(f);
        }
        Ok(())
    }

    /// Decode the next chunk and append it to `out` as interleaved f64.
    /// Returns the number of frames appended, or `Ok(0)` at end of file.
    pub fn decode_into(&mut self, out: &mut Vec<f64>) -> Result<usize, DecodeError> {
        if self.filter.is_some() {
            return self.decode_filtered(out);
        }
        self.decode_raw(out)
    }

    /// Decode through the tempo and silence filters.
    fn decode_filtered(&mut self, out: &mut Vec<f64>) -> Result<usize, DecodeError> {
        // The filter's output format never changes: if the input does, the
        // graph is rebuilt to convert the new input to the old output.
        let ch = self
            .filter
            .as_ref()
            .map(|f| f.out_format.channels as usize)
            .unwrap_or(1);
        loop {
            // Drain whatever the filter has ready.
            let f = self.filter.as_mut().unwrap();
            let start = out.len();
            loop {
                match f.graph.get("out").unwrap().sink().frame(&mut f.out) {
                    Ok(()) => {
                        let n = f.out.samples() * ch;
                        let data = f.out.data(0);
                        for i in 0..n {
                            let b = &data[i * 8..i * 8 + 8];
                            out.push(f64::from_ne_bytes(b.try_into().unwrap()));
                        }
                    }
                    Err(ffmpeg::Error::Eof) => {
                        self.done = true;
                        break;
                    }
                    Err(_) => break, // EAGAIN: needs more input
                }
            }
            if out.len() > start {
                return Ok((out.len() - start) / ch);
            }
            if self.done {
                return Ok(0);
            }
            // Feed it one decoded frame, or end of stream.
            let n = self.decode_raw_frame()?;
            let f = self.filter.as_mut().unwrap();
            if n == 0 {
                if !f.flushed {
                    f.flushed = true;
                    let _ = f.graph.get("in").unwrap().source().flush();
                }
                continue;
            }
            let in_fmt = frame_format(&self.frame);
            if in_fmt != self.format {
                log::info!(
                    "stream format changed to {} ch {} Hz, converting",
                    in_fmt.channels,
                    in_fmt.rate
                );
                let tb = self.input.stream(self.stream_index).unwrap().time_base();
                let old = self.filter.take().unwrap();
                let mut f =
                    Filter::new(self.frame.format(), in_fmt, old.out_format, tb, &self.opts)?;
                f.src_end = old.src_end;
                self.filter = Some(f);
                self.format = in_fmt;
            }
            let ts = self.frame.timestamp();
            let tb = self.time_base;
            let rate = self.format.rate as f64;
            let samples = self.frame.samples();
            let f = self.filter.as_mut().unwrap();
            f.src_end = match ts {
                Some(t) => t as f64 * tb + samples as f64 / rate,
                None => f.src_end + samples as f64 / rate,
            };
            // Push the frame through right away, keeping our own reference,
            // the way the ffmpeg tool feeds its graphs.
            let mut src = f.graph.get("in").unwrap();
            // SAFETY: both pointers are valid; KEEP_REF leaves our frame intact.
            let r = unsafe {
                ffmpeg::ffi::av_buffersrc_add_frame_flags(
                    src.as_mut_ptr(),
                    self.frame.as_mut_ptr(),
                    ffmpeg::ffi::AV_BUFFERSRC_FLAG_KEEP_REF as i32
                        | ffmpeg::ffi::AV_BUFFERSRC_FLAG_PUSH as i32,
                )
            };
            if r < 0 {
                return Err(ffmpeg::Error::from(r).into());
            }
        }
    }

    /// Decode one frame into the codec's frame buffer, returning its
    /// sample count (0 at end of stream). Used to feed the filter.
    fn decode_raw_frame(&mut self) -> Result<usize, DecodeError> {
        if std::mem::take(&mut self.primed) {
            return Ok(self.frame.samples());
        }
        loop {
            if self.done || self.eof_drained {
                return Ok(0);
            }
            match self.decoder.receive_frame(&mut self.frame) {
                Ok(()) => {
                    if self.frame.samples() > 0 {
                        return Ok(self.frame.samples());
                    }
                    continue;
                }
                Err(ffmpeg::Error::Eof) => {
                    self.eof_drained = true;
                    return Ok(0);
                }
                Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::util::error::EAGAIN => {}
                Err(e) => return Err(e.into()),
            }
            if self.eof_sent {
                self.eof_drained = true;
                return Ok(0);
            }
            self.feed_packet()?;
        }
    }

    fn feed_packet(&mut self) -> Result<(), DecodeError> {
        loop {
            match self.packet.read(&mut self.input) {
                Ok(()) => {
                    if self.packet.stream() != self.stream_index {
                        continue;
                    }
                    match self.decoder.send_packet(&self.packet) {
                        Ok(()) => return Ok(()),
                        Err(ffmpeg::Error::InvalidData) => {
                            log::warn!("skipping corrupt packet");
                            continue;
                        }
                        Err(e) => return Err(e.into()),
                    }
                }
                Err(ffmpeg::Error::Eof) => {
                    let _ = self.decoder.send_eof();
                    self.eof_sent = true;
                    return Ok(());
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    fn decode_raw(&mut self, out: &mut Vec<f64>) -> Result<usize, DecodeError> {
        if std::mem::take(&mut self.primed) {
            let n = self.append_frame(out)?;
            if n > 0 {
                return Ok(n);
            }
        }
        loop {
            if self.done {
                return Ok(0);
            }
            match self.decoder.receive_frame(&mut self.frame) {
                Ok(()) => {
                    let n = self.append_frame(out)?;
                    if n > 0 {
                        return Ok(n);
                    }
                    continue;
                }
                Err(ffmpeg::Error::Eof) => {
                    self.done = true;
                    return Ok(0);
                }
                Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::util::error::EAGAIN => {}
                Err(e) => return Err(e.into()),
            }

            if self.eof_sent {
                // Decoder wants more but there is nothing left.
                self.done = true;
                return Ok(0);
            }

            // Feed the decoder one packet from our stream. A corrupt
            // packet is skipped rather than ending the track.
            self.feed_packet()?;
        }
    }

    fn append_frame(&mut self, out: &mut Vec<f64>) -> Result<usize, DecodeError> {
        if self.frame.samples() == 0 {
            return Ok(0);
        }
        let now = frame_format(&self.frame);
        if now != self.format {
            // Broadcast streams do this: an ad break at another rate, or
            // HE-AAC switching parametric stereo on. Follow it, and let the
            // decoder thread open a new output for the new format.
            if now.channels == 0 || now.channels > MAX_CHANNELS || now.rate == 0 {
                return Err(DecodeError::Channels(now.channels));
            }
            log::info!(
                "stream format changed from {} ch {} Hz to {} ch {} Hz",
                self.format.channels,
                self.format.rate,
                now.channels,
                now.rate
            );
            // Each decode returns one frame's worth, so the caller's buffer
            // never holds both formats.
            debug_assert!(out.is_empty(), "format change inside one batch");
            self.format = now;
            self.info.sample_rate = now.rate;
            self.info.channels = now.channels;
            self.format_changed = true;
        }
        let frame = &self.frame;
        let samples = frame.samples();
        let ch = self.format.channels as usize;

        // Work out how many leading frames to drop after a seek.
        let mut skip = 0usize;
        if let Some(target) = self.skip_until {
            if let Some(ts) = frame.timestamp() {
                let start = ts as f64 * self.time_base;
                let end = start + samples as f64 / self.format.rate as f64;
                if end <= target {
                    return Ok(0);
                }
                skip = (((target - start) * self.format.rate as f64)
                    .round()
                    .max(0.0)) as usize;
                skip = skip.min(samples);
            }
            self.skip_until = None;
        }

        let start_len = out.len();
        out.reserve((samples - skip) * ch);
        let fmt = frame.format();
        let planar = matches!(
            fmt,
            Sample::U8(SampleType::Planar)
                | Sample::I16(SampleType::Planar)
                | Sample::I32(SampleType::Planar)
                | Sample::I64(SampleType::Planar)
                | Sample::F32(SampleType::Planar)
                | Sample::F64(SampleType::Planar)
        );

        macro_rules! convert {
            ($t:ty, $size:expr, $conv:expr) => {{
                let conv = $conv;
                if planar {
                    let planes: Vec<&[u8]> = (0..ch).map(|c| frame.data(c)).collect();
                    for i in skip..samples {
                        for p in &planes {
                            let b = &p[i * $size..i * $size + $size];
                            out.push(conv(<$t>::from_ne_bytes(b.try_into().unwrap())));
                        }
                    }
                } else {
                    let data = frame.data(0);
                    for i in (skip * ch)..(samples * ch) {
                        let b = &data[i * $size..i * $size + $size];
                        out.push(conv(<$t>::from_ne_bytes(b.try_into().unwrap())));
                    }
                }
            }};
        }

        match fmt {
            Sample::U8(_) => convert!(u8, 1, |v: u8| (v as f64 - 128.0) / 128.0),
            Sample::I16(_) => convert!(i16, 2, |v: i16| v as f64 / 32768.0),
            Sample::I32(_) => convert!(i32, 4, |v: i32| v as f64 / 2147483648.0),
            Sample::I64(_) => convert!(i64, 8, |v: i64| v as f64 / 9223372036854775808.0),
            Sample::F32(_) => convert!(f32, 4, |v: f32| v as f64),
            Sample::F64(_) => convert!(f64, 8, |v: f64| v),
            other => return Err(DecodeError::Format(other)),
        }

        Ok((out.len() - start_len) / ch)
    }
}

/// Rate, channels and layout of a decoded frame.
fn frame_format(frame: &ffmpeg::frame::Audio) -> StreamFormat {
    // SAFETY: reading the layout of a valid decoded frame; the mask member
    // of the union only for native order.
    let (channels, native, mask) = unsafe {
        let l = &(*frame.as_ptr()).ch_layout;
        let native = l.order == ffmpeg::ffi::AVChannelOrder::AV_CHANNEL_ORDER_NATIVE;
        (
            l.nb_channels.max(0) as u16,
            native,
            if native { l.u.mask } else { 0 },
        )
    };
    let mask = if native && mask.count_ones() == channels as u32 {
        mask
    } else {
        ffmpeg::ChannelLayout::default(channels as i32).bits()
    };
    StreamFormat {
        rate: frame.rate(),
        channels,
        mask,
    }
}

fn is_int(fmt: Sample) -> bool {
    matches!(
        fmt,
        Sample::U8(_) | Sample::I16(_) | Sample::I32(_) | Sample::I64(_)
    )
}

/// Channel mask for a decoder's layout, or FFmpeg's default for the count
/// when the file doesn't say (or uses a custom order we can't map).
fn channel_mask(decoder: &ffmpeg::decoder::Audio, channels: u16) -> u64 {
    // SAFETY: reading the order tag and, for native order only, the mask
    // member of the union, on a valid open codec context.
    let (native, mask) = unsafe {
        let l = &(*decoder.as_ptr()).ch_layout;
        let native = l.order == ffmpeg::ffi::AVChannelOrder::AV_CHANNEL_ORDER_NATIVE;
        (native, if native { l.u.mask } else { 0 })
    };
    if native && mask.count_ones() == channels as u32 {
        mask
    } else {
        ffmpeg::ChannelLayout::default(channels as i32).bits()
    }
}

/// Pull the title out of an ICY metadata block:
/// `StreamTitle='Artist - Title';StreamUrl='';`
pub fn parse_stream_title(packet: &str) -> Option<String> {
    let start = packet.find("StreamTitle='")? + "StreamTitle='".len();
    let rest = &packet[start..];
    // Titles can contain apostrophes; the field ends at "';".
    let end = rest
        .find("';")
        .or_else(|| rest.rfind('\''))
        .unwrap_or(rest.len());
    let t = rest[..end].trim().trim_end_matches('\'').trim();
    (!t.is_empty() && t != "-").then(|| t.to_string())
}

impl Filter {
    /// A graph from `input` (decoded frames in `sample_fmt`) to packed f64
    /// in `output`. The two differ only when a stream changed format.
    fn new(
        sample_fmt: Sample,
        input: StreamFormat,
        output: StreamFormat,
        tb: ffmpeg::Rational,
        opts: &OpenOptions,
    ) -> Result<Self, DecodeError> {
        let mut graph = ffmpeg::filter::Graph::new();
        let args = format!(
            "time_base={}/{}:sample_rate={}:sample_fmt={}:channel_layout=0x{:x}",
            tb.numerator(),
            tb.denominator().max(1),
            input.rate,
            sample_fmt.name(),
            input.mask
        );
        let abuffer = ffmpeg::filter::find("abuffer").ok_or(DecodeError::NoAudio)?;
        let sink = ffmpeg::filter::find("abuffersink").ok_or(DecodeError::NoAudio)?;
        graph.add(&abuffer, "in", &args)?;
        graph.add(&sink, "out", "")?;

        let mut chain: Vec<String> = Vec::new();
        if opts.trim_silence {
            // Cut any silence over 0.7 s down to 0.25 s, so pauses in speech
            // still read as pauses.
            chain.push(
                "silenceremove=stop_periods=-1:stop_duration=0.7:stop_threshold=-45dB:stop_silence=0.25"
                    .into(),
            );
        }
        // 0 means "not set", which is normal speed.
        let tempo = if opts.tempo <= 0.0 {
            1.0
        } else {
            opts.tempo.clamp(0.5, 3.0)
        };
        if (tempo - 1.0).abs() > 1e-3 {
            // atempo sounds best inside 0.5 to 2; chain two for more.
            if tempo > 2.0 {
                chain.push("atempo=2.0".into());
                chain.push(format!("atempo={:.4}", tempo / 2.0));
            } else {
                chain.push(format!("atempo={tempo:.4}"));
            }
        }
        chain.push(format!(
            "aformat=sample_fmts=dbl:channel_layouts=0x{:x}:sample_rates={}",
            output.mask, output.rate
        ));
        graph
            .output("in", 0)?
            .input("out", 0)?
            .parse(&chain.join(","))?;
        graph.validate()?;
        Ok(Self {
            graph,
            out: ffmpeg::frame::Audio::empty(),
            out_format: output,
            src_end: 0.0,
            flushed: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icy_titles() {
        assert_eq!(
            parse_stream_title("StreamTitle='Nirvana - Blew';StreamUrl='';").as_deref(),
            Some("Nirvana - Blew")
        );
        assert_eq!(
            parse_stream_title("StreamTitle='Guns N' Roses - Rocket Queen';").as_deref(),
            Some("Guns N' Roses - Rocket Queen")
        );
        assert_eq!(parse_stream_title("StreamTitle='';"), None);
        assert_eq!(parse_stream_title("garbage"), None);
    }

    #[test]
    fn network_detection() {
        assert!(is_network("https://x/stream"));
        assert!(is_network("HTTP://x"));
        assert!(!is_network("/home/zack/Music/a.flac"));
    }
}
