//! DLNA renderers (UPnP MediaRenderer): network speakers, AV receivers,
//! smart TVs, Kodi, gmrender. Found with an SSDP search like the media
//! servers in [`crate::remote::dlna`], and told what to play with their
//! AVTransport service: SetAVTransportURI with the stream's URL, then Play.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::{Kind, Target};
use crate::remote::dlna::{ssdp_group, ssdp_search};
use crate::remote::http;

const RENDERER: &str = "urn:schemas-upnp-org:device:MediaRenderer:1";
const AVT: &str = "urn:schemas-upnp-org:service:AVTransport:1";

/// Renderers that answer within `wait`.
pub fn discover(wait: Duration) -> Vec<Target> {
    let mut out: Vec<Target> = Vec::new();
    for loc in ssdp_search(ssdp_group(), RENDERER, wait) {
        if let Ok((name, _)) = describe(&loc)
            && !out.iter().any(|t| t.addr == loc)
        {
            out.push(Target {
                kind: Kind::Dlna,
                name,
                addr: loc,
            });
        }
    }
    out
}

/// A renderer's name and its absolute AVTransport control URL.
fn describe(location: &str) -> Result<(String, String), String> {
    let c = http::client().map_err(|e| e.to_string())?;
    let body = c
        .get(location)
        .timeout(Duration::from_secs(5))
        .send()
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.text())
        .map_err(|e| e.to_string())?;
    parse_description(&body, location)
}

fn parse_description(body: &str, location: &str) -> Result<(String, String), String> {
    let doc = roxmltree::Document::parse(body).map_err(|e| format!("description: {e}"))?;
    let text = |node: roxmltree::Node, name: &str| -> Option<String> {
        node.descendants()
            .find(|n| n.tag_name().name() == name)
            .and_then(|n| n.text())
            .map(|t| t.trim().to_string())
    };
    let root = doc.root_element();
    let device = root
        .descendants()
        .find(|n| {
            n.tag_name().name() == "device"
                && text(*n, "deviceType").is_some_and(|t| t.contains("MediaRenderer"))
        })
        .ok_or("that isn't a media renderer")?;
    let service = device
        .descendants()
        .find(|n| {
            n.tag_name().name() == "service"
                && text(*n, "serviceType").is_some_and(|t| t.contains(":AVTransport:"))
        })
        .ok_or("it has no AVTransport")?;
    let control = text(service, "controlURL").unwrap_or_default();
    let base = text(root, "URLBase").filter(|b| !b.is_empty());
    let base = url::Url::parse(base.as_deref().unwrap_or(location)).map_err(|e| e.to_string())?;
    let control = base.join(&control).map_err(|e| e.to_string())?.to_string();
    let name = text(device, "friendlyName").unwrap_or_else(|| "DLNA renderer".into());
    Ok((name, control))
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The DIDL-Lite a renderer gets with the URL, which some need to know
/// it's audio they can play.
fn didl(url: &str, title: &str) -> String {
    format!(
        "<DIDL-Lite xmlns=\"urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/\" \
         xmlns:dc=\"http://purl.org/dc/elements/1.1/\" \
         xmlns:upnp=\"urn:schemas-upnp-org:metadata-1-0/upnp/\">\
         <item id=\"patina\" parentID=\"0\" restricted=\"1\">\
         <dc:title>{}</dc:title><upnp:artist>PatinaPlayer</upnp:artist>\
         <upnp:class>object.item.audioItem.musicTrack</upnp:class>\
         <res protocolInfo=\"http-get:*:audio/wav:DLNA.ORG_OP=00;DLNA.ORG_CI=0;\
         DLNA.ORG_FLAGS=01700000000000000000000000000000\">{}</res></item></DIDL-Lite>",
        xml_escape(title),
        xml_escape(url)
    )
}

fn soap(control: &str, action: &str, args: &str) -> Result<(), String> {
    let body = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
         <s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" \
         s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\"><s:Body>\
         <u:{action} xmlns:u=\"{AVT}\"><InstanceID>0</InstanceID>{args}</u:{action}>\
         </s:Body></s:Envelope>"
    );
    let c = http::client().map_err(|e| e.to_string())?;
    let r = c
        .post(control)
        .timeout(Duration::from_secs(8))
        .header("Content-Type", "text/xml; charset=\"utf-8\"")
        .header("SOAPACTION", format!("\"{AVT}#{action}\""))
        .body(body)
        .send()
        .map_err(|e| e.to_string())?;
    if r.status().is_success() {
        return Ok(());
    }
    let status = r.status();
    let text = r.text().unwrap_or_default();
    let detail = roxmltree::Document::parse(&text).ok().and_then(|d| {
        d.descendants()
            .find(|n| n.tag_name().name() == "errorDescription")
            .and_then(|n| n.text())
            .map(str::to_string)
    });
    Err(format!(
        "{action} failed: {}",
        detail.unwrap_or_else(|| status.to_string())
    ))
}

/// Point the renderer at the stream and start it, then wait for `stop`
/// and stop it.
pub(super) fn run(t: &Target, url: &str, title: &str, stop: &AtomicBool) -> Result<(), String> {
    let (_, control) = describe(&t.addr)?;
    // Whatever it was playing first, or some refuse a new URI.
    let _ = soap(&control, "Stop", "");
    soap(
        &control,
        "SetAVTransportURI",
        &format!(
            "<CurrentURI>{}</CurrentURI><CurrentURIMetaData>{}</CurrentURIMetaData>",
            xml_escape(url),
            xml_escape(&didl(url, title))
        ),
    )?;
    soap(&control, "Play", "<Speed>1</Speed>")?;
    log::info!("cast: {} is playing the stream", t.name);
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(200));
    }
    let _ = soap(&control, "Stop", "");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_renderer_description_gives_its_avtransport() {
        let xml = r#"<?xml version="1.0"?>
<root xmlns="urn:schemas-upnp-org:device-1-0">
  <device>
    <deviceType>urn:schemas-upnp-org:device:MediaRenderer:1</deviceType>
    <friendlyName>Living Room</friendlyName>
    <serviceList>
      <service>
        <serviceType>urn:schemas-upnp-org:service:RenderingControl:1</serviceType>
        <controlURL>/rc/control</controlURL>
      </service>
      <service>
        <serviceType>urn:schemas-upnp-org:service:AVTransport:1</serviceType>
        <controlURL>/avt/control</controlURL>
      </service>
    </serviceList>
  </device>
</root>"#;
        let (name, control) =
            parse_description(xml, "http://10.0.0.9:1400/xml/device.xml").unwrap();
        assert_eq!(name, "Living Room");
        assert_eq!(control, "http://10.0.0.9:1400/avt/control");
    }

    #[test]
    fn a_server_is_not_a_renderer() {
        let xml = r#"<root><device><deviceType>urn:schemas-upnp-org:device:MediaServer:1</deviceType></device></root>"#;
        assert!(parse_description(xml, "http://x/").is_err());
    }

    #[test]
    fn the_metadata_is_escaped() {
        let d = didl("http://h/stream.wav?a=1&b=2", "Rock & Roll");
        assert!(d.contains("Rock &amp; Roll"));
        assert!(d.contains("a=1&amp;b=2"));
    }
}
