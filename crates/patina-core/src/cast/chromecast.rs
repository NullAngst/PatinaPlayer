//! Telling a Chromecast to play the stream: its Default Media Receiver,
//! loaded with the stream's URL as a live audio stream.

use std::borrow::Cow;
use std::sync::atomic::{AtomicBool, Ordering};

use rust_cast::channels::heartbeat::HeartbeatResponse;
use rust_cast::channels::media::{Media, Metadata, MusicTrackMediaMetadata, StreamType};
use rust_cast::channels::receiver::CastDeviceApp;
use rust_cast::{CastDevice, ChannelMessage};

use super::Target;

const RECEIVER: &str = "receiver-0";

/// Connect, start the receiver on the stream, and keep the connection
/// alive until `stop`. Then close the receiver app.
pub(super) fn run(t: &Target, url: &str, title: &str, stop: &AtomicBool) -> Result<(), String> {
    let (host, port) = match t.addr.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse().unwrap_or(8009)),
        None => (t.addr.clone(), 8009),
    };
    let e = |e: rust_cast::errors::Error| e.to_string();
    // Chromecasts present a certificate of Google's own, which no system
    // store trusts.
    let dev = CastDevice::connect_without_host_verification(host, port).map_err(e)?;
    dev.connection.connect(RECEIVER).map_err(e)?;
    dev.heartbeat.ping().map_err(e)?;
    let app = dev
        .receiver
        .launch_app(&CastDeviceApp::DefaultMediaReceiver)
        .map_err(e)?;
    let transport: Cow<'static, str> = Cow::Owned(app.transport_id.clone());
    let session: Cow<'static, str> = Cow::Owned(app.session_id.clone());
    dev.connection.connect(transport.clone()).map_err(e)?;
    let media = Media {
        content_id: url.to_string(),
        stream_type: StreamType::Live,
        content_type: "audio/wav".into(),
        metadata: Some(Metadata::MusicTrack(MusicTrackMediaMetadata {
            title: Some(title.to_string()),
            artist: Some("PatinaPlayer".into()),
            ..Default::default()
        })),
        duration: None,
    };
    dev.media
        .load(transport, session.clone(), &media)
        .map_err(e)?;
    log::info!("cast: {} is playing the stream", t.name);
    // The device pings every few seconds and drops a sender that
    // doesn't answer.
    while !stop.load(Ordering::Relaxed) {
        match dev.receive() {
            Ok(ChannelMessage::Heartbeat(HeartbeatResponse::Ping)) => {
                dev.heartbeat.pong().map_err(e)?;
            }
            Ok(_) => {}
            Err(err) => return Err(e(err)),
        }
    }
    let _ = dev.receiver.stop_app(session);
    Ok(())
}
