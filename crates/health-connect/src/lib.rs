//! The only crate that opens network connections (plan §2.2, `scripts/check-no-net.sh`). For now it downloads
//! the OCR components the person asked to install. Requests carry no user information; certificates are
//! checked against the operating system's trust store.

use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::Path;

#[derive(Debug)]
pub enum Error {
    /// Couldn't reach the server, or it refused
    Network(String),
    /// The file arrived but isn't the pinned one (size or SHA-256): discarded
    Integrity(String),
    Io(std::io::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Network(m) => write!(f, "network: {m}"),
            Error::Integrity(m) => write!(f, "integrity: {m}"),
            Error::Io(e) => write!(f, "io: {e}"),
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

fn agent() -> ureq::Agent {
    let tls = ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build();
    ureq::Agent::config_builder()
        .tls_config(tls)
        .user_agent("health-desktop")
        // each address gets 15 s to connect, so an unreachable one (often IPv6) gives way to the next
        .timeout_connect(Some(std::time::Duration::from_secs(15)))
        .timeout_global(Some(std::time::Duration::from_secs(1800)))
        .build()
        .into()
}

/// Downloads `url` to `dest` only if it is exactly `bytes` long with SHA-256 `sha256`. Written to a
/// temporary file next to `dest` first, so a broken download never leaves a half file under the real name.
/// `progress(done, total)` as bytes arrive.
pub fn download(url: &str, dest: &Path, sha256: &str, bytes: u64, progress: &mut dyn FnMut(u64, u64)) -> Result<(), Error> {
    let mut res = agent().get(url).call().map_err(|e| Error::Network(e.to_string()))?;
    let mut body = res.body_mut().with_config().limit(bytes + 1).reader();
    let tmp = dest.with_extension("part");
    let mut out = std::fs::File::create(&tmp)?;
    let mut hash = Sha256::new();
    let mut done = 0u64;
    let mut buf = vec![0u8; 1 << 16];
    let result = (|| -> Result<(), Error> {
        loop {
            let n = body.read(&mut buf).map_err(|e| Error::Network(e.to_string()))?;
            if n == 0 {
                break;
            }
            done += n as u64;
            if done > bytes {
                return Err(Error::Integrity(format!("{url}: larger than expected")));
            }
            hash.update(&buf[..n]);
            out.write_all(&buf[..n])?;
            progress(done, bytes);
        }
        out.sync_all()?;
        let got = hex::encode(hash.finalize_reset());
        if done != bytes || got != sha256 {
            return Err(Error::Integrity(format!("{url}: got {done} bytes, sha256 {got}")));
        }
        Ok(())
    })();
    match result {
        Ok(()) => {
            std::fs::rename(&tmp, dest)?;
            Ok(())
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}
