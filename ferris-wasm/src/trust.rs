//! Publisher trust for app packages: signed manifests and operator grants.
//!
//! An app package is a directory with `plugin.toml`, `<app>.wasm` and `world/`.
//! This module gives that package an *authenticated* description:
//!
//! * [`AppManifest`] is the subset of the package the publisher signs — name,
//!   version, publisher, key id, the capabilities/hosts/programs the app asks
//!   for, and the SHA-256 of the component. Its [`AppManifest::canonical_bytes`]
//!   is a length-framed, fixed-order serialization; the lists are sorted and
//!   de-duplicated first, so two manifests that only differ in list order sign
//!   the same bytes.
//! * [`PublisherKey`] / [`TrustStore`] are the operator's allow-list of signing
//!   keys, loadable from TOML or JSON.
//! * [`sign_manifest`] and [`verify_manifest`] use ring's Ed25519. Verification
//!   also proves that the `wasm_sha256` the publisher signed is the hash of the
//!   component in the package ([`AppManifest::verify_wasm_file`]).
//! * [`GrantSet`] is what an operator approves. The manifest is the *request*;
//!   `grants.toml` (written by `ferris app install`) is the approval, and the
//!   host checks the grant, never the manifest.
//!
//! ## On-disk formats
//!
//! Signature, written by `ferris app sign` next to `plugin.toml`:
//!
//! ```text
//! app.sig   # base64 of the 64-byte Ed25519 signature over canonical_bytes()
//! ```
//!
//! The trust fields travel in `plugin.toml` (so they are part of the signed
//! manifest). `ferris app sign` writes `key_id`; the publisher declares the
//! rest:
//!
//! ```toml
//! name = "facturacion"
//! version = "1.2.3"
//! publisher = "Acme"
//! key_id = "ed25519:0123456789abcdef0123456789abcdef"
//! capabilities = ["process"]
//! http_hosts = ["api.example.com"]
//! process_bins = ["bin/convert"]
//! ```
//!
//! Trust store (`--trust`), TOML or JSON:
//!
//! ```toml
//! [[keys]]
//! key_id = "ed25519:0123456789abcdef0123456789abcdef"
//! public_key_base64 = "…32 raw Ed25519 public-key bytes in base64…"
//! ```
//!
//! Approved grants (`grants.toml`, written at the root of the installed app —
//! `apps/<app>/grants.toml` in the catalogue, with a copy at
//! `apps/<app>/world/grants.toml` that travels into each site instance, and
//! `plugins/<app>/grants.toml` in the legacy flat layout):
//!
//! ```toml
//! capabilities = ["process"]
//! http_hosts = ["api.example.com"]
//! process_bins = ["bin/convert"]
//! ```
//!
//! No secret is stored in any of these files: they carry public keys, public
//! signatures and the operator's decisions.

use std::borrow::Cow;
use std::path::Path;

use base64::Engine as _;
use ring::signature::{ED25519, Ed25519KeyPair, KeyPair as _, UnparsedPublicKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The signature file `ferris app sign` writes next to `plugin.toml`.
pub const SIGNATURE_FILE: &str = "app.sig";

/// The operator-approved grants file written at the root of an installed app.
pub const GRANTS_FILE: &str = "grants.toml";

/// The app manifest parsed from `plugin.toml`.
pub const PLUGIN_MANIFEST: &str = "plugin.toml";

/// One key the operator trusts to sign apps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublisherKey {
    /// Stable id derived from the public key; also stored in `plugin.toml`.
    pub key_id: String,
    /// 32 raw Ed25519 public-key bytes, standard base64.
    pub public_key_base64: String,
}

/// The operator's trusted publisher keys.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustStore {
    #[serde(default)]
    pub keys: Vec<PublisherKey>,
}

impl TrustStore {
    /// Parse a TOML trust store.
    ///
    /// # Errors
    ///
    /// [`TrustError::Parse`] when the text is not a valid trust store.
    pub fn from_toml(text: &str) -> Result<Self, TrustError> {
        toml::from_str(text).map_err(|error| TrustError::Parse(error.to_string()))
    }

    /// Parse a JSON trust store.
    ///
    /// # Errors
    ///
    /// [`TrustError::Parse`] when the text is not a valid trust store.
    pub fn from_json(text: &str) -> Result<Self, TrustError> {
        serde_json::from_str(text).map_err(|error| TrustError::Parse(error.to_string()))
    }

    /// Read a trust store, choosing the format from the extension (`.json` is
    /// JSON, everything else is TOML).
    ///
    /// # Errors
    ///
    /// [`TrustError::Io`] when the file cannot be read, plus the parse error of
    /// [`TrustStore::from_toml`]/[`TrustStore::from_json`].
    pub fn from_path(path: &Path) -> Result<Self, TrustError> {
        let text = std::fs::read_to_string(path)?;
        if path
            .extension()
            .and_then(std::ffi::OsStr::to_str)
            .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
        {
            Self::from_json(&text)
        } else {
            Self::from_toml(&text)
        }
    }

    /// The key with `key_id`, if the operator trusts it.
    #[must_use]
    pub fn get(&self, key_id: &str) -> Option<&PublisherKey> {
        self.keys.iter().find(|key| key.key_id == key_id)
    }
}

/// The publisher-signed description of a package.
///
/// Lists are sorted and de-duplicated by [`AppManifest::normalize`] before they
/// reach the wire, so equality and canonical bytes do not depend on the order a
/// manifest happened to use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppManifest {
    pub name: String,
    pub version: String,
    pub publisher: String,
    pub key_id: String,
    pub capabilities: Vec<String>,
    pub http_hosts: Vec<String>,
    pub process_bins: Vec<String>,
    /// Lower-case hex SHA-256 of `<app>.wasm`.
    pub wasm_sha256: String,
}

/// The subset of `plugin.toml` the trust layer reads.
///
/// Unknown keys belong to the catalogue/descriptor and are ignored on purpose.
#[derive(Debug, Deserialize)]
struct ManifestFile {
    name: String,
    #[serde(default = "default_version")]
    version: String,
    #[serde(default)]
    publisher: String,
    #[serde(default)]
    key_id: String,
    #[serde(default)]
    capabilities: Vec<String>,
    #[serde(default)]
    http_hosts: Vec<String>,
    #[serde(default)]
    process_bins: Vec<String>,
}

fn default_version() -> String {
    "0.0.0".to_string()
}

impl AppManifest {
    /// Build a manifest, normalizing every list.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        version: impl Into<String>,
        publisher: impl Into<String>,
        key_id: impl Into<String>,
        capabilities: Vec<String>,
        http_hosts: Vec<String>,
        process_bins: Vec<String>,
        wasm_sha256: impl Into<String>,
    ) -> Self {
        let mut manifest = Self {
            name: name.into(),
            version: version.into(),
            publisher: publisher.into(),
            key_id: key_id.into(),
            capabilities,
            http_hosts,
            process_bins,
            wasm_sha256: wasm_sha256.into().to_ascii_lowercase(),
        };
        manifest.normalize();
        manifest
    }

    /// Sort and de-duplicate every list, so canonical bytes never depend on
    /// insertion order.
    pub fn normalize(&mut self) {
        normalize_list(&mut self.capabilities);
        normalize_list(&mut self.http_hosts);
        normalize_list(&mut self.process_bins);
        self.wasm_sha256.make_ascii_lowercase();
    }

    /// Read `plugin.toml` and hash `<name>.wasm` in `dir`.
    ///
    /// # Errors
    ///
    /// [`TrustError::Io`] when the manifest or component cannot be read,
    /// [`TrustError::Manifest`] when `plugin.toml` does not carry a usable name
    /// or does not parse.
    pub fn from_package(dir: &Path) -> Result<Self, TrustError> {
        let manifest_path = dir.join(PLUGIN_MANIFEST);
        let text = std::fs::read_to_string(&manifest_path)?;
        let file: ManifestFile = toml::from_str(&text).map_err(|error| {
            TrustError::Manifest(format!("{}: {error}", manifest_path.display()))
        })?;
        if !ferris_core::plugins::is_valid_app_name(&file.name) {
            return Err(TrustError::Manifest(format!(
                "{} has an unusable app name `{}`: use letters, digits, `-` or `_`",
                manifest_path.display(),
                file.name
            )));
        }
        let wasm = dir.join(format!("{}.wasm", file.name));
        let wasm_sha256 = sha256_file(&wasm)?;
        Ok(Self::new(
            file.name,
            file.version,
            file.publisher,
            file.key_id,
            file.capabilities,
            file.http_hosts,
            file.process_bins,
            wasm_sha256,
        ))
    }

    /// Prove the component on disk is the one the manifest describes.
    ///
    /// # Errors
    ///
    /// [`TrustError::Io`] when the component cannot be read and
    /// [`TrustError::WasmHashMismatch`] when its digest differs from
    /// `wasm_sha256`.
    pub fn verify_wasm_file(&self, dir: &Path) -> Result<(), TrustError> {
        let wasm = dir.join(format!("{}.wasm", self.name));
        let actual = sha256_file(&wasm)?;
        if actual == self.wasm_sha256 {
            Ok(())
        } else {
            Err(TrustError::WasmHashMismatch {
                expected: self.wasm_sha256.clone(),
                actual,
            })
        }
    }

    /// The canonical bytes the publisher signs.
    ///
    /// The format is deliberately not `serde_json`: every field is written in a
    /// fixed order with a length prefix, and every list is sorted first. A
    /// value can therefore never shift a boundary, and two equal manifests
    /// always produce identical bytes.
    #[must_use]
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"ferris-app-manifest-v1\n");
        put(&mut out, "name", &self.name);
        put(&mut out, "version", &self.version);
        put(&mut out, "publisher", &self.publisher);
        put(&mut out, "key_id", &self.key_id);
        put(&mut out, "wasm_sha256", &self.wasm_sha256);
        put_list(&mut out, "capabilities", &self.capabilities);
        put_list(&mut out, "http_hosts", &self.http_hosts);
        put_list(&mut out, "process_bins", &self.process_bins);
        out
    }

    /// The grants this manifest asks the operator to approve.
    #[must_use]
    pub fn grants(&self) -> GrantSet {
        GrantSet {
            capabilities: self.capabilities.clone(),
            http_hosts: self.http_hosts.clone(),
            process_bins: self.process_bins.clone(),
        }
    }
}

fn normalize_list(list: &mut Vec<String>) {
    list.sort();
    list.dedup();
}

fn put(out: &mut Vec<u8>, label: &str, value: &str) {
    out.extend_from_slice(label.as_bytes());
    out.push(b'=');
    out.extend_from_slice(value.len().to_string().as_bytes());
    out.push(b':');
    out.extend_from_slice(value.as_bytes());
    out.push(b'\n');
}

fn put_list(out: &mut Vec<u8>, label: &str, values: &[String]) {
    let mut sorted: Vec<&str> = values.iter().map(String::as_str).collect();
    sorted.sort_unstable();
    sorted.dedup();
    out.extend_from_slice(label.as_bytes());
    out.push(b'=');
    out.extend_from_slice(sorted.len().to_string().as_bytes());
    out.push(b'\n');
    for value in sorted {
        put(out, "item", value);
    }
}

/// What an operator approved for an installed app.
///
/// This is the value stored in `grants.toml` and the only thing the host should
/// consult. The manifest remains the app's request.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrantSet {
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub http_hosts: Vec<String>,
    #[serde(default)]
    pub process_bins: Vec<String>,
}

impl GrantSet {
    /// Build a grant set, normalizing every list.
    #[must_use]
    pub fn new(
        capabilities: Vec<String>,
        http_hosts: Vec<String>,
        process_bins: Vec<String>,
    ) -> Self {
        let mut grants = Self {
            capabilities,
            http_hosts,
            process_bins,
        };
        grants.normalize();
        grants
    }

    /// Sort and de-duplicate every list.
    pub fn normalize(&mut self) {
        normalize_list(&mut self.capabilities);
        normalize_list(&mut self.http_hosts);
        normalize_list(&mut self.process_bins);
    }

    /// Whether the operator approved nothing at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.capabilities.is_empty() && self.http_hosts.is_empty() && self.process_bins.is_empty()
    }

    /// Whether `capability` was approved.
    #[must_use]
    pub fn allows_capability(&self, capability: &str) -> bool {
        self.capabilities.iter().any(|entry| entry == capability)
    }

    /// Whether outbound HTTP to `host` was approved. Matching is exact: a
    /// wildcard would have to become a policy language, not a string compare.
    #[must_use]
    pub fn allows_http_host(&self, host: &str) -> bool {
        self.http_hosts.iter().any(|entry| entry == host)
    }

    /// Whether `program` (a name or a path relative to the app world) was
    /// approved.
    #[must_use]
    pub fn allows_process_bin(&self, program: &str) -> bool {
        let program = program.trim().trim_start_matches("./");
        self.process_bins
            .iter()
            .any(|entry| entry.trim().trim_start_matches("./") == program)
    }

    /// Parse `grants.toml`.
    ///
    /// # Errors
    ///
    /// [`TrustError::Parse`] when the text is not a grant set.
    pub fn from_toml(text: &str) -> Result<Self, TrustError> {
        let mut grants: Self =
            toml::from_str(text).map_err(|error| TrustError::Parse(error.to_string()))?;
        grants.normalize();
        Ok(grants)
    }

    /// Serialize as `grants.toml` content.
    ///
    /// # Errors
    ///
    /// [`TrustError::Parse`] if TOML serialization fails (it does not for this
    /// shape, but the caller still does not get to ignore it).
    pub fn to_toml(&self) -> Result<String, TrustError> {
        toml::to_string_pretty(self).map_err(|error| TrustError::Parse(error.to_string()))
    }

    /// Read a grants file. A missing file is an empty grant set, because an app
    /// installed before trust existed has not been approved for anything.
    ///
    /// # Errors
    ///
    /// [`TrustError::Io`] for a read failure other than "not found", plus
    /// [`GrantSet::from_toml`] parse errors.
    pub fn load(path: &Path) -> Result<Self, TrustError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::from_toml(&text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(TrustError::Io(error)),
        }
    }

    /// Write `grants.toml`, atomically (temporary file + rename).
    ///
    /// # Errors
    ///
    /// [`TrustError::Io`] on any write/rename failure, plus
    /// [`GrantSet::to_toml`].
    pub fn save(&self, path: &Path) -> Result<(), TrustError> {
        let body = self.to_toml()?;
        let text = format!(
            "# Approved by the operator with `ferris app install`.\n\
             # The host checks this file, not the app manifest.\n{body}"
        );
        let temporary = path.with_extension("toml.tmp");
        std::fs::write(&temporary, text)?;
        std::fs::rename(&temporary, path)?;
        Ok(())
    }
}

/// A signing key: an Ed25519 PKCS#8 key pair plus the id derived from it.
pub struct SigningKey {
    key: Ed25519KeyPair,
}

impl SigningKey {
    /// Load a PKCS#8 Ed25519 key. DER and PEM are both accepted; the PEM path
    /// only strips the armor, it does not invent another format.
    ///
    /// # Errors
    ///
    /// [`TrustError::InvalidKey`] when the bytes are not a PKCS#8 Ed25519 key.
    pub fn from_pkcs8(bytes: &[u8]) -> Result<Self, TrustError> {
        let der = decode_pkcs8(bytes)?;
        let key = Ed25519KeyPair::from_pkcs8(der.as_ref())
            .map_err(|error| TrustError::InvalidSigningKey(error.to_string()))?;
        Ok(Self { key })
    }

    /// Read a PKCS#8 key file.
    ///
    /// # Errors
    ///
    /// [`TrustError::Io`] when the file cannot be read, plus
    /// [`SigningKey::from_pkcs8`].
    pub fn from_pkcs8_file(path: &Path) -> Result<Self, TrustError> {
        Self::from_pkcs8(&std::fs::read(path)?)
    }

    /// The key id: `ed25519:` plus the first 16 bytes of SHA-256 over the raw
    /// public key, lower-case hex. Deriving it from the key means the id in
    /// `plugin.toml` cannot point at a different key without breaking the
    /// signature.
    #[must_use]
    pub fn key_id(&self) -> String {
        key_id_for(self.key.public_key().as_ref())
    }

    /// The raw public key, standard base64 (what a trust entry needs).
    #[must_use]
    pub fn public_key_base64(&self) -> String {
        base64::engine::general_purpose::STANDARD.encode(self.key.public_key().as_ref())
    }

    /// Sign bytes, returning standard base64.
    #[must_use]
    pub fn sign_base64(&self, message: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(self.key.sign(message).as_ref())
    }
}

/// Sign `manifest`'s canonical bytes.
///
/// # Errors
///
/// [`TrustError::Manifest`] when the manifest has no key id (a signature must be
/// attributable to a key) or no wasm hash.
pub fn sign_manifest(
    manifest: &AppManifest,
    signing_key: &SigningKey,
) -> Result<String, TrustError> {
    validate_manifest_for_signing(manifest)?;
    if manifest.key_id != signing_key.key_id() {
        return Err(TrustError::Manifest(format!(
            "manifest key id `{}` does not match the signing key `{}`",
            manifest.key_id,
            signing_key.key_id()
        )));
    }
    Ok(signing_key.sign_base64(&manifest.canonical_bytes()))
}

/// A manifest that passed signature, trust-store and key-id checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedManifest {
    pub manifest: AppManifest,
    pub key_id: String,
    pub publisher: String,
    pub grants: GrantSet,
}

impl VerifiedManifest {
    /// The grants the operator must still approve at install time.
    #[must_use]
    pub fn requested_grants(&self) -> &GrantSet {
        &self.grants
    }
}

/// Verify `signature_b64` over `manifest` under the key `manifest.key_id`.
///
/// The signature covers the wasm digest, so callers must build the manifest
/// from the package on disk ([`AppManifest::from_package`]) and, when they have
/// the directory, call [`AppManifest::verify_wasm_file`] as well.
///
/// # Errors
///
/// [`TrustError::Unsigned`] for an empty key id,
/// [`TrustError::UnknownKey`] when the operator does not trust the id,
/// [`TrustError::InvalidKey`]/[`TrustError::InvalidSignatureEncoding`] for
/// malformed material, and [`TrustError::InvalidSignature`] when verification
/// fails.
pub fn verify_manifest(
    manifest: &AppManifest,
    signature_b64: &str,
    trust_store: &TrustStore,
) -> Result<VerifiedManifest, TrustError> {
    if manifest.key_id.trim().is_empty() {
        return Err(TrustError::Unsigned);
    }
    if manifest.wasm_sha256.len() != 64
        || !manifest
            .wasm_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(TrustError::Manifest(
            "wasm_sha256 must be 64 hexadecimal characters".to_string(),
        ));
    }
    let trusted = trust_store
        .get(&manifest.key_id)
        .ok_or_else(|| TrustError::UnknownKey(manifest.key_id.clone()))?;
    let public_key = base64::engine::general_purpose::STANDARD
        .decode(trusted.public_key_base64.trim())
        .map_err(|error| TrustError::InvalidKey {
            key_id: trusted.key_id.clone(),
            detail: error.to_string(),
        })?;
    if public_key.len() != 32 {
        return Err(TrustError::InvalidKey {
            key_id: trusted.key_id.clone(),
            detail: format!("expected 32 bytes, got {}", public_key.len()),
        });
    }
    let signature = decode_signature(signature_b64)?;
    UnparsedPublicKey::new(&ED25519, &public_key)
        .verify(&manifest.canonical_bytes(), &signature)
        .map_err(|_| TrustError::InvalidSignature(manifest.key_id.clone()))?;
    Ok(VerifiedManifest {
        manifest: manifest.clone(),
        key_id: manifest.key_id.clone(),
        publisher: manifest.publisher.clone(),
        grants: manifest.grants(),
    })
}

/// Verify the package at `dir`: rebuild the manifest from `plugin.toml` and the
/// component, then check `app.sig` against `trust_store`.
///
/// # Errors
///
/// [`TrustError::Unsigned`] when `app.sig` is missing, plus
/// [`AppManifest::from_package`], [`AppManifest::verify_wasm_file`] and
/// [`verify_manifest`].
pub fn verify_package(
    dir: &Path,
    trust_store: &TrustStore,
) -> Result<VerifiedManifest, TrustError> {
    let manifest = AppManifest::from_package(dir)?;
    manifest.verify_wasm_file(dir)?;
    let signature = read_signature(dir)?;
    verify_manifest(&manifest, &signature, trust_store)
}

/// Read a base64 signature file. Accepts an optional `ed25519:` prefix and
/// surrounding whitespace.
///
/// # Errors
///
/// [`TrustError::Unsigned`] when the file is missing or empty, plus
/// [`TrustError::Io`] for other read failures.
pub fn read_signature(dir: &Path) -> Result<String, TrustError> {
    let path = dir.join(SIGNATURE_FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(TrustError::Unsigned);
        }
        Err(error) => return Err(TrustError::Io(error)),
    };
    let signature = text.trim();
    let signature = signature.strip_prefix("ed25519:").unwrap_or(signature);
    if signature.is_empty() {
        return Err(TrustError::Unsigned);
    }
    Ok(signature.to_string())
}

/// Lower-case hex SHA-256 of `bytes`.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Lower-case hex SHA-256 of a file.
///
/// # Errors
///
/// [`TrustError::Io`] when the file cannot be read.
pub fn sha256_file(path: &Path) -> Result<String, TrustError> {
    let bytes = std::fs::read(path)?;
    Ok(sha256_hex(&bytes))
}

fn validate_manifest_for_signing(manifest: &AppManifest) -> Result<(), TrustError> {
    if manifest.key_id.trim().is_empty() {
        return Err(TrustError::Manifest(
            "the manifest has no key_id; run `ferris app sign` to write it".to_string(),
        ));
    }
    if manifest.wasm_sha256.len() != 64 {
        return Err(TrustError::Manifest(
            "the manifest is missing the component hash".to_string(),
        ));
    }
    Ok(())
}

fn decode_signature(signature_b64: &str) -> Result<Vec<u8>, TrustError> {
    let signature_b64 = signature_b64.trim();
    let signature_b64 = signature_b64
        .strip_prefix("ed25519:")
        .unwrap_or(signature_b64);
    let signature = base64::engine::general_purpose::STANDARD
        .decode(signature_b64)
        .map_err(|error| TrustError::InvalidSignatureEncoding(error.to_string()))?;
    if signature.len() != 64 {
        return Err(TrustError::InvalidSignatureEncoding(format!(
            "expected a 64-byte Ed25519 signature, got {} bytes",
            signature.len()
        )));
    }
    Ok(signature)
}

fn key_id_for(public_key: &[u8]) -> String {
    let digest = Sha256::digest(public_key);
    let hex: String = digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("ed25519:{hex}")
}

/// Decode a PKCS#8 PEM armor when present; DER is borrowed untouched.
fn decode_pkcs8(bytes: &[u8]) -> Result<Cow<'_, [u8]>, TrustError> {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Ok(Cow::Borrowed(bytes));
    };
    if !text.contains("-----BEGIN") {
        return Ok(Cow::Borrowed(bytes));
    }
    let body: String = text
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .collect::<Vec<_>>()
        .join("");
    let der = base64::engine::general_purpose::STANDARD
        .decode(body.trim())
        .map_err(|error| TrustError::InvalidSigningKey(format!("invalid PEM body: {error}")))?;
    Ok(Cow::Owned(der))
}

/// Errors from parsing, signing and verifying a trusted package.
#[derive(Debug, thiserror::Error)]
pub enum TrustError {
    /// A filesystem operation failed.
    #[error("cannot read or write a trust file: {0}")]
    Io(#[from] std::io::Error),
    /// The manifest or its fields are not usable.
    #[error("invalid app manifest: {0}")]
    Manifest(String),
    /// `app.sig` is absent or empty.
    #[error("the app is unsigned; pass --allow-unsigned to install it anyway")]
    Unsigned,
    /// The manifest names a key the operator does not trust.
    #[error("unknown publisher key `{0}`; add it to the trust store")]
    UnknownKey(String),
    /// A trusted public key is malformed.
    #[error("publisher key `{key_id}` is not a valid Ed25519 key: {detail}")]
    InvalidKey { key_id: String, detail: String },
    /// The private key used to sign is malformed.
    #[error("the signing key is not a valid PKCS#8 Ed25519 key: {0}")]
    InvalidSigningKey(String),
    /// The signature is not base64 or not 64 bytes.
    #[error("the signature is not a valid Ed25519 signature: {0}")]
    InvalidSignatureEncoding(String),
    /// The signature does not verify under the trusted key.
    #[error("the signature does not verify under publisher key `{0}`")]
    InvalidSignature(String),
    /// The component on disk is not the signed one.
    #[error(
        "the component hash does not match the signed manifest (expected {expected}, got {actual})"
    )]
    WasmHashMismatch { expected: String, actual: String },
    /// A TOML/JSON file does not parse.
    #[error("cannot parse a trust file: {0}")]
    Parse(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::rand::SystemRandom;

    fn key_pair() -> (SigningKey, String) {
        let rng = SystemRandom::new();
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).expect("generate pkcs8");
        let key = SigningKey::from_pkcs8(pkcs8.as_ref()).expect("valid key");
        let public = key.public_key_base64();
        (key, public)
    }

    fn trust_store(key_id: &str, public_key_base64: &str) -> TrustStore {
        TrustStore {
            keys: vec![PublisherKey {
                key_id: key_id.to_string(),
                public_key_base64: public_key_base64.to_string(),
            }],
        }
    }

    fn manifest() -> AppManifest {
        AppManifest::new(
            "facturacion",
            "1.2.3",
            "Acme",
            "",
            vec!["process".to_string(), "http-unrestricted".to_string()],
            vec!["api.example.com".to_string()],
            vec!["bin/convert".to_string(), "tool.sh".to_string()],
            sha256_hex(b"the signed component"),
        )
    }

    fn signed() -> (AppManifest, String, TrustStore) {
        let (key, public) = key_pair();
        let mut manifest = manifest();
        manifest.key_id = key.key_id();
        let signature = sign_manifest(&manifest, &key).expect("sign");
        let store = trust_store(&manifest.key_id, &public);
        (manifest, signature, store)
    }

    #[test]
    fn sign_and_verify_round_trip() {
        let (manifest, signature, store) = signed();
        let verified = verify_manifest(&manifest, &signature, &store).expect("verified");
        assert_eq!(verified.publisher, "Acme");
        assert_eq!(verified.manifest.version, "1.2.3");
        assert_eq!(verified.key_id, manifest.key_id);
        assert!(verified.grants.allows_capability("process"));
        assert!(verified.grants.allows_http_host("api.example.com"));
        assert!(verified.grants.allows_process_bin("bin/convert"));
    }

    #[test]
    fn a_tampered_manifest_fails() {
        let (mut manifest, signature, store) = signed();
        manifest.version = "9.9.9".to_string();
        let error = verify_manifest(&manifest, &signature, &store).expect_err("tampered");
        assert!(matches!(error, TrustError::InvalidSignature(_)), "{error}");
    }

    #[test]
    fn a_wrong_key_fails() {
        let (manifest, signature, _) = signed();
        let (_, other_public) = key_pair();
        // Same key id, different public key: this is a malicious/broken trust
        // entry, not a missing one, so it must fail at verification.
        let store = trust_store(&manifest.key_id, &other_public);
        let error = verify_manifest(&manifest, &signature, &store).expect_err("wrong key");
        assert!(matches!(error, TrustError::InvalidSignature(_)), "{error}");
    }

    #[test]
    fn an_unknown_key_id_fails() {
        let (mut manifest, signature, store) = signed();
        manifest.key_id = "ed25519:00000000000000000000000000000000".to_string();
        let error = verify_manifest(&manifest, &signature, &store).expect_err("unknown");
        assert!(matches!(error, TrustError::UnknownKey(_)), "{error}");
    }

    #[test]
    fn an_unsigned_manifest_fails() {
        let (mut manifest, signature, store) = signed();
        manifest.key_id.clear();
        let error = verify_manifest(&manifest, &signature, &store).expect_err("unsigned");
        assert!(matches!(error, TrustError::Unsigned), "{error}");
    }

    #[test]
    fn canonicalization_is_stable_across_list_order() {
        let mut first = manifest();
        first.capabilities = vec!["process".to_string(), "http-unrestricted".to_string()];
        first.http_hosts = vec!["api.example.com".to_string()];
        first.process_bins = vec!["tool.sh".to_string(), "bin/convert".to_string()];
        let mut second = manifest();
        second.capabilities = vec!["http-unrestricted".to_string(), "process".to_string()];
        second.http_hosts = vec!["api.example.com".to_string()];
        second.process_bins = vec!["bin/convert".to_string(), "tool.sh".to_string()];
        assert_eq!(first.canonical_bytes(), second.canonical_bytes());
        // A reordering is not a new signature: both verify with the same one.
        let (key, public) = key_pair();
        first.key_id = key.key_id();
        second.key_id = first.key_id.clone();
        let signature = sign_manifest(&first, &key).expect("sign");
        let store = trust_store(&first.key_id, &public);
        assert!(verify_manifest(&second, &signature, &store).is_ok());
    }

    #[test]
    fn a_changed_component_is_refused() {
        let dir = std::env::temp_dir().join(format!(
            "ferris-trust-hash-{}-{}",
            std::process::id(),
            uuid_like()
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join("facturacion.wasm"), b"the signed component").expect("wasm");
        std::fs::write(
            dir.join(PLUGIN_MANIFEST),
            "name = \"facturacion\"\nversion = \"1.2.3\"\npublisher = \"Acme\"\nkey_id = \"ed25519:00\"\n",
        )
        .expect("manifest");
        let manifest = AppManifest::from_package(&dir).expect("manifest");
        manifest.verify_wasm_file(&dir).expect("matches");
        std::fs::write(dir.join("facturacion.wasm"), b"a different component").expect("changed");
        let error = manifest.verify_wasm_file(&dir).expect_err("changed");
        assert!(
            matches!(error, TrustError::WasmHashMismatch { .. }),
            "{error}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_trust_store_loads_from_toml_and_json() {
        let (key, public) = key_pair();
        let key_id = key.key_id();
        let toml = format!("[[keys]]\nkey_id = \"{key_id}\"\npublic_key_base64 = \"{public}\"\n");
        let json = format!(
            "{{\"keys\":[{{\"key_id\":\"{key_id}\",\"public_key_base64\":\"{public}\"}}]}}"
        );
        assert_eq!(
            TrustStore::from_toml(&toml).expect("toml").keys[0].key_id,
            key_id
        );
        assert_eq!(
            TrustStore::from_json(&json).expect("json").keys[0].key_id,
            key_id
        );
    }

    #[test]
    fn grants_round_trip_through_toml() {
        let grants = GrantSet::new(
            vec!["process".to_string()],
            vec!["api.example.com".to_string()],
            vec!["bin/convert".to_string()],
        );
        let parsed = GrantSet::from_toml(&grants.to_toml().expect("toml")).expect("parse");
        assert_eq!(parsed, grants);
        assert!(parsed.allows_process_bin("./bin/convert"));
        assert!(!parsed.allows_process_bin("sh"));
    }

    fn uuid_like() -> u128 {
        // No uuid dependency in this crate: the process id plus a monotonic
        // timestamp is enough to keep a test directory unique.
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos())
    }
}
