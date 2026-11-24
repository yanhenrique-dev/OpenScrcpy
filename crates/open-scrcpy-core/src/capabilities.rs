//! Detects which long options the configured scrcpy binary supports.
//!
//! scrcpy has renamed and removed flags across releases, and users can point
//! the app at a custom binary. The `--help` output is the authoritative list
//! for the exact build in use; the version header is only used in messages.
//! If detection fails or looks unlike scrcpy's help, the detector becomes
//! permissive so the real scrcpy error is not masked.

use std::collections::HashSet;
use std::time::Duration;

use crate::process::{create_command, output_with_timeout};

const HELP_TIMEOUT: Duration = Duration::from_secs(5);
/// Below this count the output is not the expected scrcpy help format.
const MIN_TRUSTED_OPTIONS: usize = 20;

const KNOWN_AUDIO_SOURCES: &[&str] = &[
    "output",
    "playback",
    "mic",
    "mic-unprocessed",
    "mic-camcorder",
    "mic-voice-recognition",
    "mic-voice-communication",
    "voice-call",
    "voice-call-uplink",
    "voice-call-downlink",
    "voice-performance",
];

#[derive(Debug, Default)]
pub struct Capabilities {
    version: Option<String>,
    options: HashSet<String>,
    audio_sources: HashSet<String>,
    reliable: bool,
}

impl Capabilities {
    /// False only when the help output was recognized as scrcpy's. An
    /// unrecognized binary keeps the old permissive behavior.
    pub fn supports(&self, flag: &str) -> bool {
        !self.reliable || self.options.contains(flag)
    }

    pub fn supports_audio_source(&self, source: &str) -> bool {
        !self.reliable || self.audio_sources.contains(source)
    }

    pub fn version(&self) -> &str {
        self.version.as_deref().unwrap_or("unknown")
    }

    /// Test constructor for a detected capability set.
    pub fn for_test(options: &[&str], audio_sources: &[&str]) -> Self {
        Self {
            version: Some("5.0.1".to_string()),
            options: options.iter().map(|option| (*option).to_string()).collect(),
            audio_sources: audio_sources
                .iter()
                .map(|source| (*source).to_string())
                .collect(),
            reliable: true,
        }
    }
}

/// Runs `scrcpy --help` and parses the supported long options.
pub async fn detect(exe_path: &str) -> Capabilities {
    let mut command = create_command(exe_path);
    command.arg("--help");
    let output = match output_with_timeout(command, HELP_TIMEOUT).await {
        Ok(output) => output,
        Err(error) => {
            eprintln!("[scrcpy] could not detect capabilities: {error}");
            return Capabilities::default();
        }
    };

    let mut help = String::from_utf8_lossy(&output.stdout).into_owned();
    help.push_str(&String::from_utf8_lossy(&output.stderr));
    let (options, version) = parse_help(&help);
    let reliable = options.len() >= MIN_TRUSTED_OPTIONS;
    let audio_sources = if reliable {
        parse_audio_sources(&help)
    } else {
        HashSet::new()
    };
    Capabilities {
        version,
        options,
        audio_sources,
        reliable,
    }
}

/// Removes unsupported long options from an argument vector, including the
/// following token for options that take a separate value. Returns the kept
/// arguments and the names of the skipped options.
pub fn filter_args(args: Vec<String>, caps: &Capabilities) -> (Vec<String>, Vec<String>) {
    if !caps.reliable {
        return (args, Vec::new());
    }

    let mut kept = Vec::with_capacity(args.len());
    let mut skipped = Vec::new();
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        let Some(rest) = arg.strip_prefix("--") else {
            kept.push(arg);
            continue;
        };
        let name = rest.split('=').next().unwrap_or(rest);
        if caps.supports(name) {
            kept.push(arg);
        } else {
            skipped.push(name.to_string());
            if !arg.contains('=') && takes_separate_value(name) {
                let _ = iter.next();
            }
        }
    }
    (kept, skipped)
}

pub fn skipped_message(caps: &Capabilities, skipped: &[String]) -> Option<String> {
    if skipped.is_empty() {
        return None;
    }
    let mut names = skipped.to_vec();
    names.sort();
    names.dedup();
    let flags = names
        .iter()
        .map(|name| format!("--{name}"))
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!(
        "[SYSTEM] scrcpy {} does not support: {flags}. Update scrcpy to enable them.",
        caps.version()
    ))
}

fn takes_separate_value(name: &str) -> bool {
    matches!(
        name,
        "render-driver" | "video-bit-rate" | "orientation" | "max-fps" | "camera-fps" | "max-size"
    )
}

fn parse_help(help: &str) -> (HashSet<String>, Option<String>) {
    let version = help
        .lines()
        .find_map(|line| line.strip_prefix("scrcpy "))
        .and_then(|rest| rest.split_whitespace().next())
        .map(|version| version.trim_start_matches('v').to_string());
    let options = help
        .lines()
        .filter_map(option_name)
        .map(str::to_string)
        .collect();
    (options, version)
}

/// Returns the long option on a help header line. Headers have four leading
/// spaces (`    --flag`, `    -f, --flag`); descriptions have eight and must
/// not be mistaken for options because they often mention flags in prose.
fn option_name(line: &str) -> Option<&str> {
    let line = line.trim_end_matches('\r');
    let rest = line.strip_prefix("    ")?;
    if rest.starts_with(' ') {
        return None;
    }
    let rest = rest.split_once(", ").map_or(rest, |(_, long)| long);
    let long = rest.strip_prefix("--")?;
    // scrcpy prints optional values as `--new-display[=[<w>x<h>][/<dpi>]]`;
    // stop at the first character that cannot belong to an option name.
    let name = long
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
        .next()?;
    (!name.is_empty()).then_some(name)
}

fn parse_audio_sources(help: &str) -> HashSet<String> {
    let Some(block) = option_block(help, "audio-source") else {
        return HashSet::new();
    };
    KNOWN_AUDIO_SOURCES
        .iter()
        .filter(|source| contains_token(&block, source))
        .map(|source| (*source).to_string())
        .collect()
}

fn option_block(help: &str, flag: &str) -> Option<String> {
    let mut block = String::new();
    let mut in_block = false;
    for line in help.lines() {
        if let Some(name) = option_name(line) {
            if in_block {
                break;
            }
            if name == flag {
                in_block = true;
                continue;
            }
        }
        if in_block {
            block.push_str(line);
            block.push('\n');
        }
    }
    in_block.then_some(block)
}

fn contains_token(haystack: &str, needle: &str) -> bool {
    haystack.match_indices(needle).any(|(idx, _)| {
        let before = haystack[..idx].chars().next_back();
        let after = haystack[idx + needle.len()..].chars().next();
        is_token_boundary(before) && is_token_boundary(after)
    })
}

fn is_token_boundary(character: Option<char>) -> bool {
    character.is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_' && c != '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    const HELP: &str = concat!(
        "scrcpy 5.0.1 <https://github.com/Genymobile/scrcpy>\r\n",
        "\r\n",
        "    -f, --fullscreen\r\n",
        "        Make scrcpy window fullscreen.\r\n",
        "    --audio-source=source\r\n",
        "        Possible values: \"output\", \"mic\", \"mic-voice-communication\".\r\n",
        "    --camera-size=<width>x<height>\r\n",
        "        Select the camera size.\r\n",
        "    --new-display[=[<width>x<height>][/<dpi>]]\r\n",
        "        Create a new display.\r\n",
        "    -G\r\n",
    );

    #[test]
    fn parses_version_options_and_audio_sources() {
        let (options, version) = parse_help(HELP);
        assert_eq!(version.as_deref(), Some("5.0.1"));
        assert!(options.contains("fullscreen"));
        assert!(options.contains("audio-source"));
        assert!(options.contains("camera-size"));
        // The bracket for the optional value must not become part of the name.
        assert!(options.contains("new-display"));
        // A short-only option and a description mention are not long options.
        assert!(!options.contains("G"));
        assert!(!options.contains("output"));

        let sources = parse_audio_sources(HELP);
        assert!(sources.contains("output"));
        assert!(sources.contains("mic"));
        assert!(sources.contains("mic-voice-communication"));
    }

    #[test]
    fn unknown_help_is_permissive() {
        let caps = Capabilities::default();
        assert!(caps.supports("anything"));
        assert!(caps.supports_audio_source("mic-voice-communication"));

        let args = vec!["--new-display=100x200".to_string()];
        let (kept, skipped) = filter_args(args, &caps);
        assert_eq!(kept, vec!["--new-display=100x200"]);
        assert!(skipped.is_empty());
    }

    #[tokio::test]
    #[ignore = "requires a real scrcpy binary (TEST_SCRCPY_PATH)"]
    async fn detects_a_real_scrcpy_help() {
        let path = std::env::var("TEST_SCRCPY_PATH").unwrap_or_else(|_| "scrcpy".to_string());
        let caps = detect(&path).await;
        assert!(caps.reliable, "help output was not recognized");
        assert!(caps.supports("video-codec"));
        assert!(caps.supports("audio-source"));
        assert!(caps.supports_audio_source("mic"));
        assert!(caps.supports_audio_source("mic-voice-communication"));
    }

    #[test]
    fn filter_drops_unsupported_flags_and_their_values() {
        let caps = Capabilities::for_test(&["video-bit-rate"], &["mic"]);
        let args = vec![
            "-s".to_string(),
            "device".to_string(),
            "--video-bit-rate".to_string(),
            "8M".to_string(),
            "--background-color=#fff".to_string(),
            "--new-display=100x200".to_string(),
        ];
        let (kept, skipped) = filter_args(args, &caps);
        assert_eq!(kept, vec!["-s", "device", "--video-bit-rate", "8M"]);
        assert_eq!(skipped, vec!["background-color", "new-display"]);
    }
}
