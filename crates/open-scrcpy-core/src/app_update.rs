//! Native app-update manifest: parsing, version check and signed download.
//!
//! The GPUI app does not use the Tauri updater. It reads a separate manifest
//! (`latest-gpui.json`) that points at the native artifacts and carries a
//! detached Minisign signature plus a SHA-256 for each asset. Nothing here
//! installs an update; the shell owns restart/replace policy.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use minisign_verify::{PublicKey, Signature};
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// Minisign public key reused from the 2.x updater, so the native manifest can
/// be signed with the same release key.
pub const RELEASE_PUBLIC_KEY: &str = "RWQcCyCyg6vilZ9J2/3pY++KjuRNgKr8DKtLosMXfhf0iCAgiHpMHxhv";

#[derive(Debug, Clone, Deserialize)]
pub struct PlatformAsset {
    /// Detached Minisign signature published beside the artifact.
    pub signature: String,
    pub url: String,
    /// Lowercase hex SHA-256 of the artifact.
    #[serde(default)]
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateManifest {
    pub version: String,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub pub_date: Option<String>,
    pub platforms: BTreeMap<String, PlatformAsset>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateStatus {
    UpToDate,
    Available { version: String, notes: String },
}

#[derive(Debug)]
pub enum UpdateError {
    InvalidManifest(String),
    InvalidUrl(String),
    MissingPlatform(String),
    InvalidVersion(String),
    Signature(String),
    Checksum(String),
    Io(String),
    Network(String),
}

impl fmt::Display for UpdateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidManifest(message) => {
                write!(formatter, "invalid update manifest: {message}")
            }
            Self::InvalidUrl(url) => write!(formatter, "invalid update URL: {url}"),
            Self::MissingPlatform(target) => {
                write!(formatter, "manifest has no asset for {target}")
            }
            Self::InvalidVersion(message) => write!(formatter, "invalid version: {message}"),
            Self::Signature(message) => write!(formatter, "signature rejected: {message}"),
            Self::Checksum(message) => write!(formatter, "checksum rejected: {message}"),
            Self::Io(message) => write!(formatter, "io error: {message}"),
            Self::Network(message) => write!(formatter, "network error: {message}"),
        }
    }
}

impl std::error::Error for UpdateError {}

impl UpdateManifest {
    pub fn parse(json: &str) -> Result<Self, UpdateError> {
        let manifest: Self = serde_json::from_str(json)
            .map_err(|error| UpdateError::InvalidManifest(error.to_string()))?;

        semver::Version::parse(manifest.version.trim())
            .map_err(|error| UpdateError::InvalidVersion(error.to_string()))?;
        if manifest.platforms.is_empty() {
            return Err(UpdateError::InvalidManifest(
                "platforms must not be empty".to_string(),
            ));
        }
        for (target, asset) in &manifest.platforms {
            if target.trim().is_empty() {
                return Err(UpdateError::InvalidManifest(
                    "platform target must not be empty".to_string(),
                ));
            }
            if !asset.url.starts_with("https://") {
                return Err(UpdateError::InvalidUrl(asset.url.clone()));
            }
            if let Some(sha256) = &asset.sha256 {
                if sha256.len() != 64 || !sha256.chars().all(|c| c.is_ascii_hexdigit()) {
                    return Err(UpdateError::Checksum(
                        "sha256 must be 64 hex characters".to_string(),
                    ));
                }
            }
        }
        Ok(manifest)
    }

    pub fn asset_for(&self, target: &str) -> Result<&PlatformAsset, UpdateError> {
        self.platforms
            .get(target)
            .ok_or_else(|| UpdateError::MissingPlatform(target.to_string()))
    }
}

/// The manifest key for the running platform.
pub fn current_target() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows-x86_64"
    } else if cfg!(target_os = "macos") {
        if cfg!(target_arch = "aarch64") {
            "darwin-aarch64"
        } else {
            "darwin-x86_64"
        }
    } else if cfg!(target_os = "linux") {
        "linux-x86_64"
    } else {
        "unknown"
    }
}

/// True when `candidate` is a newer release than `current`.
pub fn is_newer(candidate: &str, current: &str) -> Result<bool, UpdateError> {
    let candidate = semver::Version::parse(candidate.trim())
        .map_err(|error| UpdateError::InvalidVersion(error.to_string()))?;
    let current = semver::Version::parse(current.trim())
        .map_err(|error| UpdateError::InvalidVersion(error.to_string()))?;
    Ok(candidate > current)
}

pub fn check(
    manifest_json: &str,
    current_version: &str,
    target: &str,
) -> Result<(UpdateStatus, Option<PlatformAsset>), UpdateError> {
    let manifest = UpdateManifest::parse(manifest_json)?;
    if !is_newer(&manifest.version, current_version)? {
        return Ok((UpdateStatus::UpToDate, None));
    }
    let asset = manifest.asset_for(target)?.clone();
    Ok((
        UpdateStatus::Available {
            version: manifest.version,
            notes: manifest.notes.unwrap_or_default(),
        },
        Some(asset),
    ))
}

/// Verifies the detached Minisign signature that the release publishes beside
/// an artifact.
pub fn verify_asset(bytes: &[u8], signature: &str, public_key: &str) -> Result<(), UpdateError> {
    let public_key = PublicKey::from_base64(public_key)
        .map_err(|error| UpdateError::Signature(error.to_string()))?;
    let signature =
        Signature::decode(signature).map_err(|error| UpdateError::Signature(error.to_string()))?;
    public_key
        .verify(bytes, &signature, false)
        .map_err(|error| UpdateError::Signature(error.to_string()))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Fetches the manifest text. The caller passes the URL; this module never
/// embeds a release host.
pub async fn fetch_manifest(url: &str) -> Result<String, UpdateError> {
    if !url.starts_with("https://") {
        return Err(UpdateError::InvalidUrl(url.to_string()));
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|error| UpdateError::Network(error.to_string()))?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| UpdateError::Network(error.to_string()))?;
    if !response.status().is_success() {
        return Err(UpdateError::Network(format!(
            "manifest returned HTTP {}",
            response.status()
        )));
    }
    response
        .text()
        .await
        .map_err(|error| UpdateError::Network(error.to_string()))
}

/// Downloads an artifact, enforces the manifest SHA-256, verifies the Minisign
/// signature and writes it atomically into `directory`.
pub async fn download_verified(
    asset: &PlatformAsset,
    public_key: &str,
    directory: &Path,
    file_name: &str,
) -> Result<PathBuf, UpdateError> {
    if !asset.url.starts_with("https://") {
        return Err(UpdateError::InvalidUrl(asset.url.clone()));
    }
    if file_name.contains('/') || file_name.contains('\\') || file_name == "." || file_name == ".."
    {
        return Err(UpdateError::Io("invalid file name".to_string()));
    }
    std::fs::create_dir_all(directory).map_err(|error| UpdateError::Io(error.to_string()))?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|error| UpdateError::Network(error.to_string()))?;
    let mut response = client
        .get(&asset.url)
        .send()
        .await
        .map_err(|error| UpdateError::Network(error.to_string()))?;
    if !response.status().is_success() {
        return Err(UpdateError::Network(format!(
            "artifact returned HTTP {}",
            response.status()
        )));
    }

    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| UpdateError::Network(error.to_string()))?
    {
        bytes.extend_from_slice(&chunk);
        if bytes.len() > 300 * 1024 * 1024 {
            return Err(UpdateError::Network(
                "artifact exceeds the 300 MiB download cap".to_string(),
            ));
        }
    }

    if let Some(expected) = &asset.sha256 {
        let actual = sha256_hex(&bytes);
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(UpdateError::Checksum(format!(
                "expected {expected}, got {actual}"
            )));
        }
    }
    verify_asset(&bytes, &asset.signature, public_key)?;

    let target = directory.join(file_name);
    let temporary = directory.join(format!(".{file_name}.part"));
    std::fs::write(&temporary, &bytes).map_err(|error| UpdateError::Io(error.to_string()))?;
    std::fs::rename(&temporary, &target).map_err(|error| UpdateError::Io(error.to_string()))?;
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test vector from the minisign-verify crate documentation.
    const PUBLIC_KEY: &str = "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3";
    const SIGNATURE: &str = "untrusted comment: signature from minisign secret key\nRUQf6LRCGA9i559r3g7V1qNyJDApGip8MfqcadIgT9CuhV3EMhHoN1mGTkUidF/z7SrlQgXdy8ofjb7bNJJylDOocrCo8KLzZwo=\ntrusted comment: timestamp:1633700835\tfile:test\tprehashed\nwLMDjy9FLAuxZ3q4NlEvkgtyhrr0gtTu6KC4KBJdITbbOeAi1zBIYo0v4iTgt8jJpIidRJnp94ABQkJAgAooBQ==";

    #[test]
    fn release_public_key_is_a_valid_minisign_key() {
        PublicKey::from_base64(RELEASE_PUBLIC_KEY).expect("release key must parse");
    }

    #[test]
    fn verifies_a_known_minisign_signature() {
        verify_asset(b"test", SIGNATURE, PUBLIC_KEY).expect("known vector must verify");
        assert!(verify_asset(b"tampered", SIGNATURE, PUBLIC_KEY).is_err());
    }

    #[test]
    fn parses_and_checks_a_manifest() {
        let json = r#"{
            "version": "3.0.1",
            "notes": "native shell",
            "platforms": {
                "linux-x86_64": {
                    "signature": "sig",
                    "url": "https://example.com/openscrcpy.AppImage",
                    "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                }
            }
        }"#;
        let (status, asset) = check(json, "3.0.0", "linux-x86_64").expect("manifest must parse");
        assert_eq!(
            status,
            UpdateStatus::Available {
                version: "3.0.1".into(),
                notes: "native shell".into()
            }
        );
        assert!(asset.is_some());
        assert_eq!(
            check(json, "3.0.1", "linux-x86_64").unwrap().0,
            UpdateStatus::UpToDate
        );
        assert!(matches!(
            check(json, "3.0.0", "windows-x86_64"),
            Err(UpdateError::MissingPlatform(_))
        ));
    }

    #[test]
    fn rejects_insecure_or_malformed_manifests() {
        let insecure = r#"{"version":"3.0.0","platforms":{"linux-x86_64":{"signature":"s","url":"http://example.com/app"}}}"#;
        assert!(matches!(
            UpdateManifest::parse(insecure),
            Err(UpdateError::InvalidUrl(_))
        ));
        assert!(matches!(
            UpdateManifest::parse(r#"{"version":"","platforms":{}}"#),
            Err(UpdateError::InvalidVersion(_))
        ));
        let bad_sha = r#"{"version":"3.0.0","platforms":{"linux-x86_64":{"signature":"s","url":"https://example.com/app","sha256":"abc"}}}"#;
        assert!(matches!(
            UpdateManifest::parse(bad_sha),
            Err(UpdateError::Checksum(_))
        ));
    }

    #[test]
    fn compares_versions() {
        assert!(is_newer("3.0.1", "3.0.0").unwrap());
        assert!(!is_newer("3.0.0", "3.0.0").unwrap());
        assert!(is_newer("3.1.0-alpha.1", "3.0.0").unwrap());
        assert!(!is_newer("3.1.0-alpha.1", "3.1.0").unwrap());
        assert!(is_newer("not-a-version", "3.0.0").is_err());
    }

    #[test]
    fn hashes_are_lowercase_hex() {
        assert_eq!(
            sha256_hex(b"test"),
            "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"
        );
    }
}
