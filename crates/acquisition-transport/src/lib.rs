//! HTTPS acquisition transport. Not wired to the production worker.
//! Requires a trusted source record and a future VPN-only runtime.

use norylum_persistence::AuthorizedAcquisitionSource;
use reqwest::{
    header::{ACCEPT_ENCODING, CONTENT_ENCODING},
    redirect::Policy,
    Url,
};
use sha2::{Digest, Sha256};
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use tokio::io::AsyncWriteExt;

pub const MAX_TRANSFER_BYTES: u64 = 16 * 1024 * 1024 * 1024;
const MAX_BYTES_PER_SECOND: u64 = 8 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum TransferError {
    #[error("Source HTTPS non autorisee")]
    InvalidSource,
    #[error("Resolution DNS ou adresse IP refusee")]
    UnsafeAddress,
    #[error("Aucune IPv4 publique valide")]
    NoPublicAddress,
    #[error("Taille maximale de transfert depassee")]
    SizeLimit,
    #[error("Empreinte SHA-256 incorrecte")]
    DigestMismatch,
    #[error("Reponse HTTP invalide : {0}")]
    HttpStatus(u16),
    #[error("Encodage HTTP non autorise")]
    ContentEncoding,
    #[error("Transfert bloque ou sans donnees")]
    Stalled,
    #[error("Longueur HTTP incoherente")]
    LengthMismatch,
    #[error(transparent)]
    Network(#[from] reqwest::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Strict IPv4 policy for the first transport version.
/// IPv6 is not connected until its VPN leak tests are complete.
pub fn is_public_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();

    !(a == 0
        || a == 10
        || a == 127
        || a >= 224
        || (a == 100 && (64..=127).contains(&b))
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 0 && c == 0)
        || (a == 192 && b == 0 && c == 2)
        || (a == 192 && b == 88 && c == 99)
        || (a == 192 && b == 168)
        || (a == 198 && (18..=19).contains(&b))
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113))
}

fn validate_url(
    source: &AuthorizedAcquisitionSource,
    allowed_hosts: &[String],
) -> Result<(Url, String), TransferError> {
    let raw = source.source_url.as_str();

    if raw.len() > 2048
        || raw.bytes().any(|b| b.is_ascii_control() || b == b'\\')
        || source.expected_sha256.len() != 64
        || !source
            .expected_sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(TransferError::InvalidSource);
    }

    let url = Url::parse(raw).map_err(|_| TransferError::InvalidSource)?;

    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() == "/"
    {
        return Err(TransferError::InvalidSource);
    }

    let host = url
        .host_str()
        .ok_or(TransferError::InvalidSource)?
        .to_owned();

    if !host.is_ascii()
        || !host.contains('.')
        || !host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        || host.parse::<IpAddr>().is_ok()
        || !allowed_hosts
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(&host))
    {
        return Err(TransferError::InvalidSource);
    }

    Ok((url, host))
}

/// Resolve once, reject unsafe A records, and pin the validated addresses.
/// IPv6 answers are ignored; only validated IPv4 addresses are connected.
async fn verified_addresses(host: &str) -> Result<Vec<SocketAddr>, TransferError> {
    let resolved = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::net::lookup_host((host, 443)),
    )
    .await
    .map_err(|_| TransferError::Stalled)??;

    let mut addresses = Vec::new();

    for address in resolved {
        match address.ip() {
            IpAddr::V4(ip) if is_public_ipv4(ip) => {
                if !addresses.contains(&address) {
                    addresses.push(address);
                }
            }
            IpAddr::V4(_) => return Err(TransferError::UnsafeAddress),
            IpAddr::V6(_) => {}
        }
    }

    if addresses.is_empty() {
        return Err(TransferError::NoPublicAddress);
    }

    Ok(addresses)
}

/// A verified file is still quarantined, never ingested.
/// Dropping this handle removes the file.
pub struct VerifiedArtifact {
    path: PathBuf,
    bytes: u64,
}

impl VerifiedArtifact {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn bytes(&self) -> u64 {
        self.bytes
    }
}

impl Drop for VerifiedArtifact {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

struct VerifiedSink {
    path: PathBuf,
    file: tokio::fs::File,
    hasher: Sha256,
    expected: String,
    bytes: u64,
    limit: u64,
    disarmed: bool,
}

impl VerifiedSink {
    fn create(quarantine: &Path, expected: &str, limit: u64) -> Result<Self, TransferError> {
        if limit == 0
            || expected.len() != 64
            || !expected
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(TransferError::InvalidSource);
        }

        let meta = std::fs::symlink_metadata(quarantine)?;

        if !meta.is_dir() || meta.file_type().is_symlink() {
            return Err(TransferError::InvalidSource);
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            if meta.permissions().mode() & 0o077 != 0 {
                return Err(TransferError::InvalidSource);
            }
        }

        let root = std::fs::canonicalize(quarantine)?;
        let name = format!(".norylum-acq-{}.partial", uuid::Uuid::new_v4().simple());
        let path = root.join(name);

        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);

        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }

        let file = options.open(&path)?;

        Ok(Self {
            path,
            file: tokio::fs::File::from_std(file),
            hasher: Sha256::new(),
            expected: expected.to_owned(),
            bytes: 0,
            limit,
            disarmed: false,
        })
    }

    async fn write_chunk(&mut self, chunk: &[u8]) -> Result<(), TransferError> {
        let next = self
            .bytes
            .checked_add(chunk.len() as u64)
            .ok_or(TransferError::SizeLimit)?;

        if next > self.limit {
            return Err(TransferError::SizeLimit);
        }

        self.file.write_all(chunk).await?;
        self.hasher.update(chunk);
        self.bytes = next;
        Ok(())
    }

    async fn finish(mut self) -> Result<VerifiedArtifact, TransferError> {
        if self.bytes == 0 {
            return Err(TransferError::Stalled);
        }

        self.file.flush().await?;
        self.file.sync_all().await?;

        let actual = format!("{:x}", self.hasher.clone().finalize());

        if actual != self.expected {
            return Err(TransferError::DigestMismatch);
        }

        self.disarmed = true;

        Ok(VerifiedArtifact {
            path: self.path.clone(),
            bytes: self.bytes,
        })
    }
}

impl Drop for VerifiedSink {
    fn drop(&mut self) {
        if !self.disarmed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Not called by the worker yet. The caller must first verify its live
/// SQLite lease AND run inside a tested Gluetun network namespace.
pub async fn download_https(
    source: &AuthorizedAcquisitionSource,
    allowed_hosts: &[String],
    quarantine: &Path,
) -> Result<VerifiedArtifact, TransferError> {
    let (url, host) = validate_url(source, allowed_hosts)?;
    let addresses = verified_addresses(&host).await?;

    let client = reqwest::Client::builder()
        .https_only(true)
        .no_proxy()
        .no_gzip()
        .no_brotli()
        .no_zstd()
        .no_deflate()
        .redirect(Policy::none())
        .referer(false)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(3600))
        .pool_max_idle_per_host(0)
        .resolve_to_addrs(&host, &addresses)
        .build()?;

    let mut response = client
        .get(url)
        .header(ACCEPT_ENCODING, "identity")
        .send()
        .await?;

    if response.status() != reqwest::StatusCode::OK {
        return Err(TransferError::HttpStatus(response.status().as_u16()));
    }

    if response
        .content_length()
        .is_some_and(|n| n > MAX_TRANSFER_BYTES)
    {
        return Err(TransferError::SizeLimit);
    }

    if response
        .headers()
        .get(CONTENT_ENCODING)
        .is_some_and(|v| v.as_bytes() != b"identity")
    {
        return Err(TransferError::ContentEncoding);
    }

    let declared = response.content_length();

    let mut sink = VerifiedSink::create(quarantine, &source.expected_sha256, MAX_TRANSFER_BYTES)?;

    let start = Instant::now();

    loop {
        let chunk = tokio::time::timeout(Duration::from_secs(30), response.chunk())
            .await
            .map_err(|_| TransferError::Stalled)??;

        let Some(chunk) = chunk else {
            break;
        };

        sink.write_chunk(&chunk).await?;

        // Upper bound on average transfer throughput.
        let minimum_elapsed =
            Duration::from_secs_f64(sink.bytes as f64 / MAX_BYTES_PER_SECOND as f64);

        if let Some(pause) = minimum_elapsed.checked_sub(start.elapsed()) {
            tokio::time::sleep(pause).await;
        }
    }

    if declared.is_some_and(|n| n != sink.bytes) {
        return Err(TransferError::LengthMismatch);
    }

    sink.finish().await
}

#[cfg(test)]
mod tests;
