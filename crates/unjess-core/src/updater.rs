//! Self-Updater & Release Version Checker (`updater.rs`).
//!
//! Performs semantic version comparison, GitHub/manifest asset resolution
//! against `https://github.com/Bexx1107/Unjess-Rust`, and cryptographic
//! SHA-256 verification using `sha2::Sha256`.

use crate::version::CURRENT_VERSION;
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use std::time::Duration;

pub const DEFAULT_GITHUB_REPO: &str = "Bexx1107/Unjess-Rust";
pub const DEFAULT_RELEASES_API_URL: &str =
    "https://api.github.com/repos/Bexx1107/Unjess-Rust/releases/latest";
pub const DEFAULT_RAW_MANIFEST_URL: &str =
    "https://raw.githubusercontent.com/Bexx1107/Unjess-Rust/main/version.json";
pub const DEFAULT_SETUP_DOWNLOAD_URL: &str =
    "https://github.com/Bexx1107/Unjess-Rust/raw/main/dist/Unjess-Setup.exe";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateCheckResult {
    pub current_version: String,
    pub latest_version: String,
    pub update_available: bool,
    pub download_url: String,
    pub notes: String,
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub asset_name: String,
    #[serde(default)]
    pub source_url: String,
}

/// Computes the hex-encoded SHA-256 digest of `data` and compares it against `expected_hex`.
pub fn verify_sha256_bytes(data: &[u8], expected_hex: &str) -> Result<String, String> {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let actual_hex = hex::encode(hasher.finalize());
    let clean_expected = expected_hex
        .trim()
        .trim_start_matches("sha256:")
        .trim()
        .to_lowercase();
    if !clean_expected.is_empty() {
        if clean_expected.len() != 64 || !clean_expected.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!(
                "Invalid expected SHA-256 hex string '{}' (must be 64 hex characters)",
                expected_hex
            ));
        }
        if actual_hex != clean_expected {
            return Err(format!(
                "SHA-256 mismatch: expected {}, computed {}",
                clean_expected, actual_hex
            ));
        }
    }
    Ok(actual_hex)
}

/// Computes and verifies the SHA-256 digest of a file on disk.
pub fn verify_file_sha256(path: &Path, expected_hex: &str) -> Result<bool, String> {
    let bytes = fs::read(path).map_err(|e| format!("Failed reading {}: {}", path.display(), e))?;
    verify_sha256_bytes(&bytes, expected_hex).map(|_| true)
}

fn normalize_update_urls(server_url: &str) -> (String, String) {
    let trimmed = server_url.trim();
    if trimmed.is_empty()
        || trimmed.contains("Bexx1107/unjess")
        || trimmed.eq_ignore_ascii_case("https://github.com/Bexx1107/Unjess-Rust")
        || trimmed.eq_ignore_ascii_case("https://github.com/Bexx1107/Unjess-Rust/")
    {
        return (
            DEFAULT_RELEASES_API_URL.to_string(),
            DEFAULT_RAW_MANIFEST_URL.to_string(),
        );
    }
    if let Some(rest) = trimmed.strip_prefix("https://github.com/") {
        let parts: Vec<&str> = rest.trim_matches('/').split('/').collect();
        if parts.len() >= 2 {
            let owner_repo = format!("{}/{}", parts[0], parts[1]);
            return (
                format!("https://api.github.com/repos/{}/releases/latest", owner_repo),
                format!(
                    "https://raw.githubusercontent.com/{}/main/version.json",
                    owner_repo
                ),
            );
        }
    }
    (trimmed.to_string(), DEFAULT_RAW_MANIFEST_URL.to_string())
}

fn parse_update_manifest_json(
    json: &serde_json::Value,
    source_url: &str,
) -> UpdateCheckResult {
    let raw_tag = json
        .get("tag_name")
        .or_else(|| json.get("version"))
        .and_then(|v| v.as_str())
        .unwrap_or(CURRENT_VERSION);

    let latest = raw_tag.trim().trim_start_matches('v').to_string();
    let update_available = is_newer_semver(CURRENT_VERSION, &latest);

    let mut download_url = json
        .get("download_url")
        .or_else(|| json.get("html_url"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let mut asset_name = String::new();
    let mut asset_digest = String::new();

    if let Some(assets) = json.get("assets").and_then(|v| v.as_array()) {
        for asset in assets {
            let name = asset.get("name").and_then(|v| v.as_str()).unwrap_or("");
            if name.ends_with(".exe") || name.contains("Unjess") {
                asset_name = name.to_string();
                if let Some(dig) = asset.get("digest").and_then(|v| v.as_str()) {
                    asset_digest = dig
                        .trim_start_matches("sha256:")
                        .trim()
                        .to_lowercase();
                }
                if let Some(burl) = asset.get("browser_download_url").and_then(|v| v.as_str()) {
                    download_url = burl.to_string();
                    break;
                }
            }
        }
    }

    // Support `version.json` format (`binaries: { "Unjess.exe": "...", "Unjess-Setup.exe": "..." }` or `hashes: { ... }`)
    let mut setup_digest = String::new();
    for map_key in ["binaries", "hashes"] {
        if let Some(hashes) = json.get(map_key).and_then(|v| v.as_object()) {
            if setup_digest.is_empty() {
                if let Some(h) = hashes.get("Unjess-Setup.exe").and_then(|v| v.as_str()) {
                    setup_digest = h.trim().to_lowercase();
                }
            }
            if asset_digest.is_empty() {
                if let Some(h) = hashes
                    .get("Unjess-Setup.exe")
                    .or_else(|| hashes.get("Unjess.exe"))
                    .and_then(|v| v.as_str())
                {
                    asset_digest = h.trim().to_lowercase();
                    asset_name = "Unjess-Setup.exe".to_string();
                }
            }
            if download_url.is_empty() {
                download_url = DEFAULT_SETUP_DOWNLOAD_URL.to_string();
            }
        }
    }

    let mut notes = json
        .get("body")
        .or_else(|| json.get("notes"))
        .or_else(|| json.get("engine"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    if notes.is_empty() {
        notes = format!("Verified against {}", DEFAULT_GITHUB_REPO);
    }

    let mut sha256 = if !setup_digest.is_empty()
        && (download_url.is_empty() || download_url.ends_with("Unjess-Setup.exe"))
    {
        setup_digest
    } else {
        json.get("sha256")
            .or_else(|| json.get("checksum"))
            .and_then(|v| v.as_str())
            .map(|s| s.trim().trim_start_matches("sha256:").to_lowercase())
            .filter(|s| !s.is_empty())
            .unwrap_or(asset_digest)
    };

    if sha256.is_empty() && !notes.is_empty() {
        if let Ok(re) = Regex::new(r"\b([a-fA-F0-9]{64})\b") {
            if let Some(caps) = re.captures(&notes) {
                if let Some(m) = caps.get(1) {
                    sha256 = m.as_str().to_lowercase();
                }
            }
        }
    }

    if !download_url.is_empty()
        && !download_url.starts_with("https://")
        && !download_url.starts_with("http://")
    {
        download_url.clear();
    }

    UpdateCheckResult {
        current_version: CURRENT_VERSION.to_string(),
        latest_version: latest,
        update_available,
        download_url,
        notes,
        sha256,
        asset_name,
        source_url: source_url.to_string(),
    }
}

/// Queries `https://github.com/Bexx1107/Unjess-Rust` (GitHub Releases API with automatic fallback
/// to `main/version.json`) and checks if a newer version exists.
pub async fn check_for_updates(server_url: &str) -> Result<UpdateCheckResult, String> {
    let (primary_url, fallback_manifest_url) = normalize_update_urls(server_url);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(6))
        .user_agent("Unjess-Rust/4.0.1")
        .build()
        .map_err(|e| e.to_string())?;

    if let Ok(resp) = client.get(&primary_url).send().await {
        if resp.status().is_success() {
            if let Ok(json) = resp.json::<serde_json::Value>().await {
                return Ok(parse_update_manifest_json(&json, &primary_url));
            }
        }
    }

    // Fallback to raw `version.json` on `https://github.com/Bexx1107/Unjess-Rust` (`main` branch)
    let resp = client
        .get(&fallback_manifest_url)
        .send()
        .await
        .map_err(|e| format!("Update check request failed: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!(
            "Update endpoint {} returned HTTP {}",
            fallback_manifest_url,
            resp.status()
        ));
    }

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Invalid update manifest JSON: {}", e))?;

    Ok(parse_update_manifest_json(&json, &fallback_manifest_url))
}

/// Synchronous wrapper around `check_for_updates` for background worker threads in the Desktop UI.
pub fn check_for_updates_blocking(server_url: &str) -> Result<UpdateCheckResult, String> {
    let (primary_url, fallback_manifest_url) = normalize_update_urls(server_url);

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(6))
        .user_agent("Unjess-Rust/4.0.1")
        .build()
        .map_err(|e| e.to_string())?;

    if let Ok(resp) = client.get(&primary_url).send() {
        if resp.status().is_success() {
            if let Ok(json) = resp.json::<serde_json::Value>() {
                return Ok(parse_update_manifest_json(&json, &primary_url));
            }
        }
    }

    let resp = client
        .get(&fallback_manifest_url)
        .send()
        .map_err(|e| format!("GitHub update check failed: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!(
            "GitHub endpoint {} returned HTTP {}",
            fallback_manifest_url,
            resp.status()
        ));
    }

    let json: serde_json::Value = resp
        .json()
        .map_err(|e| format!("Invalid version.json manifest: {}", e))?;

    Ok(parse_update_manifest_json(&json, &fallback_manifest_url))
}

/// Downloads the latest `Unjess-Setup.exe` from GitHub, verifies its SHA-256 (if known),
/// and spawns the installer to automatically upgrade the application.
pub fn download_and_launch_installer_blocking(
    download_url: &str,
    expected_sha256: &str,
) -> Result<String, String> {
    let url = if download_url.is_empty() || download_url.contains("/releases") {
        DEFAULT_SETUP_DOWNLOAD_URL
    } else {
        download_url
    };

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(180))
        .user_agent("Unjess-Rust/4.0.1")
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    let resp = client
        .get(url)
        .send()
        .map_err(|e| format!("Failed to download update: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("Download failed with HTTP {}", resp.status()));
    }

    let bytes = resp.bytes().map_err(|e| format!("Failed reading download body: {}", e))?;

    if !expected_sha256.is_empty() {
        if let Err(e) = verify_sha256_bytes(&bytes, expected_sha256) {
            return Err(format!("Installer integrity check failed: {}", e));
        }
    }

    let temp_installer = std::env::temp_dir().join("Unjess-Setup.exe");
    fs::write(&temp_installer, &bytes)
        .map_err(|e| format!("Failed writing installer to disk: {}", e))?;

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        std::process::Command::new(&temp_installer)
            .creation_flags(0x00000008) // DETACHED_PROCESS
            .spawn()
            .map_err(|e| format!("Failed to spawn installer: {}", e))?;
        std::thread::sleep(Duration::from_millis(500));
        std::process::exit(0);
    }

    #[cfg(not(windows))]
    {
        Ok(format!(
            "Downloaded update installer to {}",
            temp_installer.display()
        ))
    }
}

fn is_newer_semver(current: &str, candidate: &str) -> bool {
    let parse = |s: &str| -> ((u32, u32, u32), bool) {
        let clean = s.trim().trim_start_matches('v');
        let no_build = clean.split('+').next().unwrap_or("");
        let (core, has_prerelease) = match no_build.split_once('-') {
            Some((c, pre)) => (c, !pre.trim().is_empty()),
            None => (no_build, false),
        };
        let mut parts = core
            .split('.')
            .filter(|p| !p.is_empty())
            .filter_map(|p| p.parse::<u32>().ok());
        (
            (
                parts.next().unwrap_or(0),
                parts.next().unwrap_or(0),
                parts.next().unwrap_or(0),
            ),
            has_prerelease,
        )
    };
    let (cur_ver, cur_pre) = parse(current);
    let (cand_ver, cand_pre) = parse(candidate);
    if cand_ver != cur_ver {
        cand_ver > cur_ver
    } else {
        // Stable release (cand_pre == false) is newer than pre-release of the same version (cur_pre == true)
        cur_pre && !cand_pre
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_semver_and_sha256_verification() {
        assert!(is_newer_semver("4.0.0", "4.0.1"));
        assert!(!is_newer_semver("4.0.0", "3.0.2"));
        assert!(!is_newer_semver("4.0.0", "4.0-beta.5"));
        assert!(!is_newer_semver("4.0.0", "4.0.0-alpha.1"));
        assert!(is_newer_semver("4.0.0-beta.1", "4.0.0"));

        let payload = b"unjess-rust-binary-payload";
        let digest = verify_sha256_bytes(payload, "").unwrap();
        assert_eq!(digest.len(), 64);
        assert!(verify_sha256_bytes(payload, &digest).is_ok());
        assert!(verify_sha256_bytes(payload, "not-a-valid-hex").is_err());
        assert!(verify_sha256_bytes(
            payload,
            "0000000000000000000000000000000000000000000000000000000000000000"
        )
        .is_err());

        let tmp = tempfile::tempdir().unwrap();
        let fpath = tmp.path().join("payload.bin");
        fs::write(&fpath, payload).unwrap();
        assert!(verify_file_sha256(&fpath, &digest).unwrap());
    }
}
