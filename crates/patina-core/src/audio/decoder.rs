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
use std::sync::Once;
use std::time::Duration;

use ffmpeg::format::sample::{Sample, Type as SampleType};
use ffmpeg_next as ffmpeg;

use super::StreamFormat;
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
        ffmpeg::util::log::set_level(ffmpeg::util::log::Level::Error);
    });
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
    done: bool,
    /// Frames still to drop after a seek, so the seek lands on the exact
    /// sample rather than wherever the demuxer's nearest packet was.
    skip_until: Option<f64>,
}

/// Highest channel count we pass through. FFmpeg can report more, but
/// nothing downstream knows what to do with a 24-channel file.
pub const MAX_CHANNELS: u16 = 8;

impl Source {
    pub fn open(path: &Path) -> Result<Self, DecodeError> {
        init_ffmpeg();
        let input = ffmpeg::format::input(path)?;
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
        if channels == 0 || channels > MAX_CHANNELS {
            return Err(DecodeError::Channels(channels));
        }
        let format = StreamFormat {
            rate: decoder.rate(),
            channels,
            mask: channel_mask(&decoder, channels),
        };
        let info = TrackInfo::from_input(path, &input, stream_index);

        Ok(Self {
            input,
            decoder,
            stream_index,
            time_base,
            format,
            info,
            frame: ffmpeg::frame::Audio::empty(),
            packet: ffmpeg::Packet::empty(),
            eof_sent: false,
            done: false,
            skip_until: None,
        })
    }

    pub fn format(&self) -> StreamFormat {
        self.format
    }

    pub fn info(&self) -> &TrackInfo {
        &self.info
    }

    /// Seek to `pos`. The next samples returned start exactly there.
    pub fn seek(&mut self, pos: Duration) -> Result<(), DecodeError> {
        let ts = (pos.as_secs_f64() * ffmpeg::ffi::AV_TIME_BASE as f64) as i64;
        // Seek to a keyframe at or before the target, then drop samples
        // until we reach it.
        self.input.seek(ts, ..ts)?;
        self.decoder.flush();
        self.eof_sent = false;
        self.done = false;
        self.skip_until = Some(pos.as_secs_f64());
        Ok(())
    }

    /// Decode the next chunk and append it to `out` as interleaved f64.
    /// Returns the number of frames appended, or `Ok(0)` at end of file.
    pub fn decode_into(&mut self, out: &mut Vec<f64>) -> Result<usize, DecodeError> {
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

            // Feed the decoder one packet from our stream.
            loop {
                match self.packet.read(&mut self.input) {
                    Ok(()) => {
                        if self.packet.stream() != self.stream_index {
                            continue;
                        }
                        match self.decoder.send_packet(&self.packet) {
                            Ok(()) => {}
                            // A corrupt packet should not end the track.
                            Err(ffmpeg::Error::InvalidData) => {
                                log::warn!(
                                    "skipping corrupt packet in {}",
                                    self.info.path.display()
                                );
                                continue;
                            }
                            Err(e) => return Err(e.into()),
                        }
                        break;
                    }
                    Err(ffmpeg::Error::Eof) => {
                        let _ = self.decoder.send_eof();
                        self.eof_sent = true;
                        break;
                    }
                    Err(e) => return Err(e.into()),
                }
            }
        }
    }

    fn append_frame(&mut self, out: &mut Vec<f64>) -> Result<usize, DecodeError> {
        let frame = &self.frame;
        let samples = frame.samples();
        let ch = self.format.channels as usize;
        if samples == 0 {
            return Ok(0);
        }
        if frame.channels() as usize != ch || frame.rate() != self.format.rate {
            // Mid-stream format changes happen in some broadcast captures.
            // Rather than play garbage, treat it as a decode error.
            log::warn!(
                "format changed mid-stream in {} ({} ch {} Hz)",
                self.info.path.display(),
                frame.channels(),
                frame.rate()
            );
            return Err(DecodeError::Channels(frame.channels()));
        }

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
