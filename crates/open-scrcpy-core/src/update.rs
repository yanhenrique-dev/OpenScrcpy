//! scrcpy release discovery and version comparison.

use std::time::Duration;

use serde_json::{json, Value};

use crate::scrcpy;

/// Client for GitHub release calls. The connect deadline keeps a stalled
/// TCP/TLS handshake from hanging the download/update UI forever; individual
/// requests still set their own total timeout.
pub fn http_client(user_agent: &str) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(user_agent)
        .connect_timeout(Duration::from_secs(10))
        .build()
        .map_err(|error| error.to_string())
}

/// True when `remote` is a newer dotted version than `local`.
pub fn compare_versions(local: &str, remote: &str) -> bool {
    let local_parts: Vec<u32> = local
        .split('.')
        .filter_map(|part| {
            part.chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse()
                .ok()
        })
        .collect();
    let remote_parts: Vec<u32> = remote
        .split('.')
        .filter_map(|part| {
            part.chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse()
                .ok()
        })
        .collect();

    for index in 0..std::cmp::max(local_parts.len(), remote_parts.len()) {
        let local_value = local_parts.get(index).copied().unwrap_or(0);
        let remote_value = remote_parts.get(index).copied().unwrap_or(0);
        if remote_value > local_value {
            return true;
        } else if local_value > remote_value {
            return false;
        }
    }
    false
}

/// Latest scrcpy tag from GitHub. Falls back to the `/releases/latest`
/// redirect when the unauthenticated API is rate-limited.
pub async fn latest_scrcpy_version() -> Result<String, String> {
    let client = http_client("OpenScrcpy-Updater")?;
    let api_url = "https://api.github.com/repos/Genymobile/scrcpy/releases/latest";
    let api_resp = client
        .get(api_url)
        .timeout(Duration::from_secs(15))
        .send()
        .await;

    let mut tag_name = String::new();
    let mut used_fallback = false;
    if let Ok(response) = api_resp {
        if response.status().is_success() {
            if let Ok(json) = response.json::<Value>().await {
                if let Some(tag) = json["tag_name"].as_str() {
                    tag_name = tag.trim_start_matches('v').to_string();
                }
            }
        } else if response.status() == reqwest::StatusCode::FORBIDDEN {
            used_fallback = true;
        }
    } else {
        used_fallback = true;
    }

    if used_fallback || tag_name.is_empty() {
        let redirect = client
            .get("https://github.com/Genymobile/scrcpy/releases/latest")
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .map_err(|error| format!("Fallback failed: {}", error))?;
        if let Some(tag) = redirect.url().as_str().split('/').next_back() {
            if tag.starts_with('v') || tag.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                tag_name = tag.trim_start_matches('v').to_string();
            }
        }
    }

    if tag_name.is_empty() {
        return Err("Could not determine latest version".to_string());
    }
    Ok(tag_name)
}

/// Checks the resolved local scrcpy against the latest GitHub release.
pub async fn check_scrcpy_update(custom_path: Option<&str>) -> Value {
    let Some(local_version) = scrcpy::local_version(custom_path).await else {
        return json!({
            "update_available": false,
            "local_version": null,
            "latest_version": null,
            "message": "Scrcpy not installed or not working"
        });
    };
    let latest_version = match latest_scrcpy_version().await {
        Ok(version) => version,
        Err(error) => {
            return json!({
                "update_available": false,
                "local_version": local_version,
                "latest_version": null,
                "message": format!("Could not fetch latest release: {}", error)
            });
        }
    };
    let update_available = compare_versions(&local_version, &latest_version);
    json!({
        "update_available": update_available,
        "local_version": local_version,
        "latest_version": latest_version
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_dotted_versions() {
        assert!(compare_versions("2.7", "3.0"));
        assert!(compare_versions("2.7", "2.8"));
        assert!(!compare_versions("3.0", "2.7"));
        assert!(!compare_versions("3.0", "3.0"));
        assert!(compare_versions("3", "3.0.1"));
    }
}
