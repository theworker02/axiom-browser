//! On-disk storage for a normal profile's HTTP cache.
//!
//! Layout under the cache directory:
//!
//! ```text
//! objects/<name>.body   decoded response body
//! index/<name>.json     key, headers, Vary values, freshness, TLS facts, last access
//! ```
//!
//! Writes are atomic (temporary file + rename); the body is written before its index
//! record, so a crash leaves at most an orphaned body, which the next `open` deletes.
//! Records with an unknown version, a missing body or a body of the wrong length are
//! deleted rather than served.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::cache::{now_unix_ms, CacheEntry, CacheKey};
use crate::headers::HeaderMap;
use crate::protocol::HttpProtocol;
use crate::response::{CertificateInfo, TlsInfo};

const FORMAT_VERSION: u32 = 1;

/// An index record read back from disk.
pub(crate) struct LoadedEntry {
    pub key: CacheKey,
    pub object: String,
    pub status: u16,
    pub headers: HeaderMap,
    pub vary: Vec<String>,
    pub vary_values: Vec<Option<String>>,
    pub fresh_until_unix_ms: i64,
    pub requires_validation: bool,
    pub must_revalidate: bool,
    pub tls: Option<TlsInfo>,
    pub accessed_unix_ms: i64,
    pub body_len: usize,
}

#[derive(Serialize, Deserialize)]
struct Record {
    version: u32,
    method: String,
    url: String,
    status: u16,
    headers: Vec<(String, String)>,
    vary: Vec<String>,
    vary_values: Vec<Option<String>>,
    fresh_until_unix_ms: i64,
    requires_validation: bool,
    must_revalidate: bool,
    accessed_unix_ms: i64,
    body_len: usize,
    tls: Option<TlsRecord>,
}

#[derive(Serialize, Deserialize)]
struct TlsRecord {
    alpn: String,
    version: Option<String>,
    cipher_suite: Option<String>,
    hostname: Option<String>,
    certificate: Option<CertRecord>,
}

#[derive(Serialize, Deserialize)]
struct CertRecord {
    subject: String,
    issuer: String,
    subject_alt_names: Vec<String>,
    serial_hex: String,
    not_before_unix: i64,
    not_after_unix: i64,
    sha256_fingerprint: String,
}

pub(crate) struct DiskStore {
    objects: PathBuf,
    index: PathBuf,
    seq: AtomicU64,
}

impl DiskStore {
    pub fn open(dir: &Path) -> io::Result<(Self, Vec<LoadedEntry>)> {
        let store = Self {
            objects: dir.join("objects"),
            index: dir.join("index"),
            seq: AtomicU64::new(0),
        };
        fs::create_dir_all(&store.objects)?;
        fs::create_dir_all(&store.index)?;
        let loaded = store.load();
        Ok((store, loaded))
    }

    fn body_path(&self, name: &str) -> PathBuf {
        self.objects.join(format!("{name}.body"))
    }

    fn meta_path(&self, name: &str) -> PathBuf {
        self.index.join(format!("{name}.json"))
    }

    fn new_name(&self) -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        format!(
            "{nanos:x}-{:x}-{:x}",
            std::process::id(),
            self.seq.fetch_add(1, Ordering::Relaxed)
        )
    }

    /// Write the body and index record of a new entry; returns its object name.
    pub fn write(&self, key: &CacheKey, entry: &CacheEntry) -> io::Result<String> {
        let name = self.new_name();
        write_atomic(&self.body_path(&name), &entry.body)?;
        if let Err(e) = self.write_record(&name, key, entry, entry.body.len()) {
            let _ = fs::remove_file(self.body_path(&name));
            return Err(e);
        }
        Ok(name)
    }

    /// Rewrite the index record of an existing entry (after a 304 or an access).
    pub fn write_meta(&self, name: &str, key: &CacheKey, entry: &CacheEntry) -> io::Result<()> {
        let len = fs::metadata(self.body_path(name))?.len() as usize;
        self.write_record(name, key, entry, len)
    }

    fn write_record(
        &self,
        name: &str,
        key: &CacheKey,
        entry: &CacheEntry,
        body_len: usize,
    ) -> io::Result<()> {
        let record = Record {
            version: FORMAT_VERSION,
            method: key.method.clone(),
            url: key.url.clone(),
            status: entry.status,
            headers: entry
                .headers
                .iter()
                .flat_map(|(k, vs)| vs.iter().map(move |v| (k.to_string(), v.clone())))
                .collect(),
            vary: entry.vary.clone(),
            vary_values: entry.vary_values().to_vec(),
            fresh_until_unix_ms: entry.fresh_until_unix_ms(),
            requires_validation: entry.requires_validation,
            must_revalidate: entry.must_revalidate,
            accessed_unix_ms: entry.accessed_unix_ms(),
            body_len,
            tls: entry.tls.as_ref().map(TlsRecord::from),
        };
        let json = serde_json::to_vec(&record).map_err(io::Error::other)?;
        write_atomic(&self.meta_path(name), &json)
    }

    pub fn read_body(&self, name: &str, len: usize) -> io::Result<Vec<u8>> {
        let body = fs::read(self.body_path(name))?;
        if body.len() != len {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected {len} bytes, found {}", body.len()),
            ));
        }
        Ok(body)
    }

    pub fn remove(&self, name: &str) {
        let _ = fs::remove_file(self.meta_path(name));
        let _ = fs::remove_file(self.body_path(name));
    }

    pub fn clear(&self) -> io::Result<()> {
        for dir in [&self.index, &self.objects] {
            for entry in fs::read_dir(dir)? {
                let _ = fs::remove_file(entry?.path());
            }
        }
        Ok(())
    }

    fn load(&self) -> Vec<LoadedEntry> {
        let mut out = Vec::new();
        let mut live = std::collections::HashSet::new();
        if let Ok(dir) = fs::read_dir(&self.index) {
            for entry in dir.flatten() {
                let path = entry.path();
                let Some(name) = stem_with_ext(&path, "json") else {
                    let _ = fs::remove_file(&path);
                    continue;
                };
                match self.load_one(&name) {
                    Some(e) => {
                        live.insert(name);
                        out.push(e);
                    }
                    None => self.remove(&name),
                }
            }
        }
        if let Ok(dir) = fs::read_dir(&self.objects) {
            for entry in dir.flatten() {
                let path = entry.path();
                let keep = stem_with_ext(&path, "body").is_some_and(|n| live.contains(&n));
                if !keep {
                    let _ = fs::remove_file(&path);
                }
            }
        }
        out
    }

    fn load_one(&self, name: &str) -> Option<LoadedEntry> {
        let bytes = fs::read(self.meta_path(name)).ok()?;
        let r: Record = serde_json::from_slice(&bytes).ok()?;
        if r.version != FORMAT_VERSION {
            return None;
        }
        let body_len = fs::metadata(self.body_path(name)).ok()?.len() as usize;
        if body_len != r.body_len || r.vary.len() != r.vary_values.len() {
            return None;
        }
        let mut headers = HeaderMap::new();
        for (k, v) in r.headers {
            headers.append(&k, v);
        }
        Some(LoadedEntry {
            key: CacheKey {
                method: r.method,
                url: r.url,
            },
            object: name.to_string(),
            status: r.status,
            headers,
            vary: r.vary,
            vary_values: r.vary_values,
            fresh_until_unix_ms: r.fresh_until_unix_ms,
            requires_validation: r.requires_validation,
            must_revalidate: r.must_revalidate,
            tls: r.tls.map(TlsInfo::from),
            accessed_unix_ms: r.accessed_unix_ms.min(now_unix_ms()),
            body_len,
        })
    }
}

fn stem_with_ext(path: &Path, ext: &str) -> Option<String> {
    if path.extension()?.to_str()? != ext {
        return None;
    }
    Some(path.file_stem()?.to_str()?.to_string())
}

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_data()?;
    }
    fs::rename(&tmp, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

impl From<&TlsInfo> for TlsRecord {
    fn from(t: &TlsInfo) -> Self {
        Self {
            alpn: t.alpn.as_str().to_string(),
            version: t.version.clone(),
            cipher_suite: t.cipher_suite.clone(),
            hostname: t.hostname.clone(),
            certificate: t.certificate.as_ref().map(|c| CertRecord {
                subject: c.subject.clone(),
                issuer: c.issuer.clone(),
                subject_alt_names: c.subject_alt_names.clone(),
                serial_hex: c.serial_hex.clone(),
                not_before_unix: c.not_before_unix,
                not_after_unix: c.not_after_unix,
                sha256_fingerprint: c.sha256_fingerprint.clone(),
            }),
        }
    }
}

impl From<TlsRecord> for TlsInfo {
    fn from(r: TlsRecord) -> Self {
        Self {
            alpn: HttpProtocol::from_alpn(&r.alpn),
            // Only responses received over a validated connection are ever stored.
            certificate_verified: true,
            version: r.version,
            cipher_suite: r.cipher_suite,
            hostname: r.hostname,
            certificate: r.certificate.map(|c| CertificateInfo {
                subject: c.subject,
                issuer: c.issuer,
                subject_alt_names: c.subject_alt_names,
                serial_hex: c.serial_hex,
                not_before_unix: c.not_before_unix,
                not_after_unix: c.not_after_unix,
                sha256_fingerprint: c.sha256_fingerprint,
            }),
        }
    }
}
