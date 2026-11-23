//! Diagnostic report writing.
//!
//! The shell resolves the downloads directory; this module owns the file-name
//! policy and the non-overwriting write.

use std::io::Write;
use std::path::Path;

/// Keep only the final path component of a frontend-provided name, so an
/// absolute path or `..` segments cannot escape the downloads directory.
pub fn safe_file_name(name: &str) -> Option<String> {
    Path::new(name)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .filter(|name| !name.is_empty())
}

/// Writes `content` under `downloads`, refusing to overwrite an existing file
/// or follow an existing symlink.
pub fn write_report(downloads: &Path, content: &str, name: &str) -> Result<String, String> {
    let safe_name = safe_file_name(name).ok_or_else(|| "Invalid report name".to_string())?;
    let path = downloads.join(&safe_name);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| error.to_string())?;
    file.write_all(content.as_bytes())
        .map_err(|error| error.to_string())?;
    Ok(path.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_only_the_final_component() {
        assert_eq!(
            safe_file_name("report.json"),
            Some("report.json".to_string())
        );
        assert_eq!(
            safe_file_name("/tmp/../../report.json"),
            Some("report.json".to_string())
        );
        assert_eq!(safe_file_name(""), None);
    }
}
