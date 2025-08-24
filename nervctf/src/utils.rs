//! Utility functions for directory operations and file handling
//! Provides helper functions for working with challenge files and directories

use anyhow::{anyhow, Result};
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Creates a directory if it doesn't exist
pub fn ensure_dir_exists(path: &Path) -> Result<()> {
    if !path.exists() {
        fs::create_dir_all(path)
            .map_err(|e| anyhow!("Failed to create directory {}: {}", path.display(), e))?;
    }
    Ok(())
}

/// Checks if a path is a valid challenge directory
pub fn is_valid_challenge_dir(path: &Path) -> bool {
    path.is_dir() && path.exists()
}

/// Gets all files in a directory with specific extensions
pub fn get_files_with_extensions(dir: &Path, extensions: &[&str]) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();

    if !dir.exists() || !dir.is_dir() {
        return Ok(files);
    }

    for entry in WalkDir::new(dir)
        .max_depth(3)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if path.is_file() {
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if extensions.contains(&ext) {
                    files.push(path.to_path_buf());
                }
            }
        }
    }

    Ok(files)
}

/// Finds challenge configuration files in a directory
pub fn find_challenge_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let patterns = ["challenge.yml", "challenge.yaml", "challenge.json"];
    let mut files = Vec::new();

    if !dir.exists() || !dir.is_dir() {
        return Ok(files);
    }

    for entry in WalkDir::new(dir)
        .max_depth(5)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if path.is_file() {
            if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
                if patterns.contains(&filename) {
                    files.push(path.to_path_buf());
                } else if filename.contains("challenge") {
                    // Also match files containing "challenge" in name
                    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                        if ext == "yml" || ext == "yaml" || ext == "json" {
                            files.push(path.to_path_buf());
                        }
                    }
                }
            }
        }
    }

    Ok(files)
}

/// Gets the relative path from a base directory
pub fn get_relative_path(base: &Path, target: &Path) -> Result<PathBuf> {
    target
        .strip_prefix(base)
        .map(|p| p.to_path_buf())
        .map_err(|e| anyhow!("Failed to get relative path: {}", e))
}

/// Checks if a file exists and is readable
pub fn file_exists_and_readable(path: &Path) -> bool {
    path.exists() && path.is_file() && fs::metadata(path).is_ok()
}

/// Creates a backup of a file
pub fn backup_file(path: &Path) -> Result<PathBuf> {
    if !path.exists() {
        return Err(anyhow!("File does not exist: {}", path.display()));
    }

    let backup_path = path.with_extension("backup");
    fs::copy(path, &backup_path)
        .map_err(|e| anyhow!("Failed to create backup of {}: {}", path.display(), e))?;

    Ok(backup_path)
}

/// Restores a file from backup
pub fn restore_from_backup(backup_path: &Path) -> Result<PathBuf> {
    if !backup_path.exists() {
        return Err(anyhow!(
            "Backup file does not exist: {}",
            backup_path.display()
        ));
    }

    let original_path = backup_path.with_extension("");
    fs::copy(backup_path, &original_path).map_err(|e| {
        anyhow!(
            "Failed to restore from backup {}: {}",
            backup_path.display(),
            e
        )
    })?;

    Ok(original_path)
}

/// Gets file size in human-readable format
pub fn get_file_size(path: &Path) -> Result<String> {
    let metadata = fs::metadata(path)
        .map_err(|e| anyhow!("Failed to get metadata for {}: {}", path.display(), e))?;

    let size = metadata.len();
    let units = ["B", "KB", "MB", "GB"];
    let mut size_f64 = size as f64;
    let mut unit_index = 0;

    while size_f64 >= 1024.0 && unit_index < units.len() - 1 {
        size_f64 /= 1024.0;
        unit_index += 1;
    }

    Ok(format!("{:.2} {}", size_f64, units[unit_index]))
}

/// Validates that all referenced files in a challenge exist
pub fn validate_challenge_files(challenge_dir: &Path, files: &[String]) -> Result<()> {
    for file in files {
        let file_path = challenge_dir.join(file);
        if !file_path.exists() {
            return Err(anyhow!(
                "Referenced file does not exist: {}",
                file_path.display()
            ));
        }
    }
    Ok(())
}

/// Creates a unique filename to avoid conflicts
pub fn get_unique_filename(base_dir: &Path, desired_name: &str) -> PathBuf {
    let mut counter = 1;
    let mut candidate = base_dir.join(desired_name);

    while candidate.exists() {
        let stem = candidate.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let extension = candidate.extension().and_then(|e| e.to_str()).unwrap_or("");

        let new_name = if counter == 1 {
            format!("{}-1.{}", stem, extension)
        } else {
            format!("{}-{}.{}", stem, counter, extension)
        };

        candidate = base_dir.join(new_name);
        counter += 1;
    }

    candidate
}

/// Gets directory statistics
pub fn get_dir_stats(path: &Path) -> Result<(usize, u64)> {
    let mut file_count = 0;
    let mut total_size = 0;

    if !path.exists() || !path.is_dir() {
        return Ok((0, 0));
    }

    for entry in WalkDir::new(path)
        .max_depth(5)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if path.is_file() {
            file_count += 1;
            if let Ok(metadata) = fs::metadata(path) {
                total_size += metadata.len();
            }
        }
    }

    Ok((file_count, total_size))
}


#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_find_challenge_files() -> Result<()> {
        let temp_dir = tempdir()?;
        let dir_path = temp_dir.path();

        // Create test files
        fs::write(dir_path.join("challenge.yml"), "test")?;
        fs::write(dir_path.join("other.txt"), "test")?;

        let files = find_challenge_files(dir_path)?;
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("challenge.yml"));

        Ok(())
    }

    #[test]
    fn test_get_relative_path() -> Result<()> {
        let base = Path::new("/base/dir");
        let target = Path::new("/base/dir/sub/file.txt");

        let relative = get_relative_path(base, target)?;
        assert_eq!(relative, Path::new("sub/file.txt"));

        Ok(())
    }

    #[test]
    fn test_get_unique_filename() {
        let temp_dir = tempdir().unwrap();
        let base_path = temp_dir.path();

        // Create existing file
        fs::write(base_path.join("test.txt"), "content").unwrap();

        let unique = get_unique_filename(base_path, "test.txt");
        assert!(unique.ends_with("test-1.txt"));
    }
}
