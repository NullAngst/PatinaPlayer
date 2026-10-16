//! The HTTP client every part of the app starts from.
//!
//! On Linux, HTTPS trusts the system's certificate store through the
//! platform verifier. On Android that verifier needs Java glue of its own,
//! so the Android build reads the system's CA files directly instead (the
//! same store Android's own apps trust, minus certificates a user added).

/// A blocking client builder with the app's TLS trust set up.
pub fn client_builder() -> reqwest::blocking::ClientBuilder {
    let b = reqwest::blocking::Client::builder();
    #[cfg(target_os = "android")]
    let b = b.tls_certs_only(android_roots().iter().cloned());
    b
}

/// Where Android keeps its CA certificates: the updatable module first
/// (Android 14 and later), then the system image.
#[cfg(target_os = "android")]
const ANDROID_CA_DIRS: [&str; 2] = [
    "/apex/com.android.conscrypt/cacerts",
    "/system/etc/security/cacerts",
];

#[cfg(target_os = "android")]
fn android_roots() -> &'static [reqwest::Certificate] {
    static ROOTS: std::sync::OnceLock<Vec<reqwest::Certificate>> = std::sync::OnceLock::new();
    ROOTS.get_or_init(|| {
        for dir in ANDROID_CA_DIRS {
            let v = load_roots(std::path::Path::new(dir));
            if !v.is_empty() {
                return v;
            }
        }
        log::warn!("no CA certificates found; HTTPS won't work");
        Vec::new()
    })
}

/// Every PEM certificate in a folder. Android's files are named by hash
/// (`9d520b32.0`) and carry a text dump before the PEM block, which the
/// parser skips.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn load_roots(dir: &std::path::Path) -> Vec<reqwest::Certificate> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    rd.flatten()
        .filter_map(|e| std::fs::read(e.path()).ok())
        // The parser only checks the PEM later, at connect time; anything
        // without a certificate block in it would fail every connection.
        .filter(|b| b.windows(27).any(|w| w == b"-----BEGIN CERTIFICATE-----"))
        .filter_map(|b| reqwest::Certificate::from_pem(&b).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_android_style_ca_files() {
        // Any CA from this machine's store will do, written the way
        // Android writes them: text first, then the PEM block.
        let Some(pem) = std::fs::read_dir("/etc/ssl/certs").ok().and_then(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "pem"))
                .find_map(|p| std::fs::read_to_string(p).ok())
        }) else {
            eprintln!("no system CA store here, skipping");
            return;
        };
        let d = tempfile::tempdir().unwrap();
        let text = format!("Certificate:\n    Data:\n        Version: 3 (0x2)\n{pem}");
        std::fs::write(d.path().join("9d520b32.0"), text).unwrap();
        std::fs::write(d.path().join("junk.0"), "not a certificate").unwrap();
        assert_eq!(load_roots(d.path()).len(), 1);
        assert!(load_roots(&d.path().join("missing")).is_empty());
        // The builder still makes a client.
        assert!(client_builder().build().is_ok());
    }
}
