//! The HTTP client every part of the app starts from.
//!
//! On Linux, HTTPS trusts the system's certificate store through the
//! platform verifier. On Android that verifier needs Java glue of its own,
//! so the Android build reads the system's CA files directly instead (the
//! same store Android's own apps trust, minus certificates a user added).
//!
//! FFmpeg does its own HTTPS for streams (radio, and server songs played
//! straight from the server). On Linux its GnuTLS reads the system store
//! by itself. On Android it's Mbed TLS, which has no store of its own, and
//! FFmpeg verifies certificates by default, so every HTTPS stream failed
//! with an I/O error. [`ffmpeg_ca_file`] gives it the same CAs as a file.

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

/// A PEM file of trusted CAs for FFmpeg's `ca_file` option, or None where
/// FFmpeg's TLS finds the system's store itself (everywhere but Android).
/// Built once per run, so CA updates from the system land on next start.
pub fn ffmpeg_ca_file() -> Option<&'static str> {
    #[cfg(target_os = "android")]
    {
        static FILE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
        FILE.get_or_init(|| {
            let out = crate::config::cache_dir().join("ca-bundle.pem");
            for dir in ANDROID_CA_DIRS {
                match write_bundle(std::path::Path::new(dir), &out, mbed::parses) {
                    Ok((0, _)) => continue,
                    Ok((n, skipped)) => {
                        if skipped > 0 {
                            log::info!(
                                "CA bundle: {n} from {dir}, {skipped} Mbed TLS can't read left out"
                            );
                        }
                        return Some(out.to_string_lossy().into_owned());
                    }
                    Err(e) => {
                        log::warn!("can't write the CA bundle to {}: {e}", out.display());
                        return None;
                    }
                }
            }
            log::warn!("no CA certificates found; HTTPS streams won't play");
            None
        })
        .as_deref()
    }
    #[cfg(not(target_os = "android"))]
    None
}

/// Every PEM certificate block in `dir`, written to `out` as one file.
/// `accept` gets each block (NUL-terminated, as Mbed TLS wants it) and
/// says whether it goes in. Mbed TLS refuses the whole file if one block
/// in it won't parse, so anything it can't read has to stay out.
/// Returns how many went in and how many were left out.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn write_bundle(
    dir: &std::path::Path,
    out: &std::path::Path,
    accept: impl Fn(&[u8]) -> bool,
) -> std::io::Result<(usize, usize)> {
    const BEGIN: &str = "-----BEGIN CERTIFICATE-----";
    const END: &str = "-----END CERTIFICATE-----";
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Ok((0, 0));
    };
    let mut files: Vec<_> = rd.flatten().map(|e| e.path()).collect();
    files.sort();
    let (mut bundle, mut n, mut skipped) = (String::new(), 0, 0);
    for f in files {
        let Ok(text) = std::fs::read_to_string(&f) else {
            continue;
        };
        let mut rest = text.as_str();
        while let Some(b) = rest.find(BEGIN) {
            let Some(e) = rest[b..].find(END) else { break };
            let block = &rest[b..b + e + END.len()];
            rest = &rest[b + e + END.len()..];
            let mut z = Vec::with_capacity(block.len() + 2);
            z.extend_from_slice(block.as_bytes());
            z.extend_from_slice(b"\n\0");
            if accept(&z) {
                bundle.push_str(block);
                bundle.push('\n');
                n += 1;
            } else {
                skipped += 1;
            }
        }
    }
    if n == 0 {
        return Ok((0, skipped));
    }
    if let Some(d) = out.parent() {
        std::fs::create_dir_all(d)?;
    }
    // Written beside and renamed over, so FFmpeg never reads half a file.
    let tmp = out.with_extension("pem.tmp");
    std::fs::write(&tmp, bundle)?;
    std::fs::rename(&tmp, out)?;
    Ok((n, skipped))
}

/// Asks the Mbed TLS that FFmpeg is linked against whether it can read a
/// certificate, the same call FFmpeg's `ca_file` loading makes.
#[cfg(all(target_os = "android", feature = "static-ffmpeg"))]
mod mbed {
    use std::ffi::c_int;

    /// Room for an `mbedtls_x509_crt`, which is only ever handled by
    /// pointer here. 3.x's is under 1 KiB on 64-bit.
    #[repr(C, align(16))]
    struct Crt([u8; 4096]);

    unsafe extern "C" {
        fn mbedtls_x509_crt_init(crt: *mut Crt);
        fn mbedtls_x509_crt_free(crt: *mut Crt);
        fn mbedtls_x509_crt_parse(chain: *mut Crt, buf: *const u8, len: usize) -> c_int;
    }

    /// `pem` ends in a NUL, which Mbed TLS needs to take it as PEM.
    pub fn parses(pem: &[u8]) -> bool {
        let mut crt = Box::new(Crt([0; 4096]));
        // SAFETY: crt is a zeroed, aligned buffer bigger than the struct,
        // initialised before use and freed once. pem outlives the call.
        unsafe {
            mbedtls_x509_crt_init(&mut *crt);
            let r = mbedtls_x509_crt_parse(&mut *crt, pem.as_ptr(), pem.len());
            mbedtls_x509_crt_free(&mut *crt);
            r == 0
        }
    }
}

/// Without the static FFmpeg there's no Mbed TLS to ask: take them all.
#[cfg(all(target_os = "android", not(feature = "static-ffmpeg")))]
mod mbed {
    pub fn parses(_: &[u8]) -> bool {
        true
    }
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

    #[test]
    fn bundles_pem_blocks_and_leaves_out_rejects() {
        let block = |c: char| {
            format!(
                "-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----",
                c.to_string().repeat(8)
            )
        };
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("certs");
        std::fs::create_dir(&src).unwrap();
        // Android style: a text dump first, then the block.
        std::fs::write(
            src.join("a.0"),
            format!("Certificate:\n  Data: x\n{}\n", block('A')),
        )
        .unwrap();
        std::fs::write(src.join("b.0"), format!("{}\n{}\n", block('B'), block('C'))).unwrap();
        std::fs::write(src.join("junk.0"), "not a certificate").unwrap();
        let out = d.path().join("cache/ca-bundle.pem");

        // Everything that parses goes in, NUL-terminated for the check.
        let (n, skipped) = write_bundle(&src, &out, |pem| {
            assert_eq!(pem.last(), Some(&0));
            !pem.windows(8).any(|w| w == b"BBBBBBBB")
        })
        .unwrap();
        assert_eq!((n, skipped), (2, 1));
        let text = std::fs::read_to_string(&out).unwrap();
        assert!(text.contains("AAAAAAAA") && text.contains("CCCCCCCC"));
        assert!(!text.contains("BBBBBBBB") && !text.contains("Certificate:"));
        assert!(!text.contains('\0'));

        // Nothing usable: no file, and the caller tries the next folder.
        let empty = d.path().join("none.pem");
        assert_eq!(write_bundle(&src, &empty, |_| false).unwrap(), (0, 3));
        assert!(!empty.exists());
        assert_eq!(
            write_bundle(&d.path().join("missing"), &empty, |_| true).unwrap(),
            (0, 0)
        );
    }
}
