//! Directory scanner for auto-detecting CTFd challenges
//! Recursively searches for challenge configuration files in current directory

use crate::ctfd_api::models::Challenge;
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Configuration for directory scanning
#[derive(Debug, Serialize, Deserialize)]
pub struct ScannerConfig {
    /// File patterns to search for
    pub patterns: Vec<String>,
    /// Maximum search depth
    pub max_depth: usize,
    /// Whether to follow symlinks
    pub follow_symlinks: bool,
    /// File extensions to consider
    pub extensions: Vec<String>,
}

impl Default for ScannerConfig {
    fn default() -> Self {
        Self {
            patterns: vec![
                "challenge.yml".to_string(),
                "challenge.yaml".to_string(),
                "challenge.json".to_string(),
            ],
            max_depth: 5,
            follow_symlinks: false,
            extensions: vec!["yml".to_string(), "yaml".to_string(), "json".to_string()],
        }
    }
}

/// Scans directories for challenge configuration files
pub struct DirectoryScanner {
    config: ScannerConfig,
}

impl DirectoryScanner {
    /// Creates a new directory scanner with default configuration
    pub fn new() -> Self {
        Self {
            config: ScannerConfig::default(),
        }
    }

    /// Creates a new directory scanner with custom configuration
    pub fn with_config(config: ScannerConfig) -> Self {
        Self { config }
    }

    /// Scans a directory for challenge files
    pub fn scan_directory(&self, base_path: &Path) -> Result<Vec<Challenge>> {
        let mut challenges = Vec::new();

        if !base_path.exists() {
            return Err(anyhow!("Directory does not exist: {}", base_path.display()));
        }

        if !base_path.is_dir() {
            return Err(anyhow!("Path is not a directory: {}", base_path.display()));
        }

        println!("🔍 Scanning directory: {}", base_path.display());

        for entry in WalkDir::new(base_path)
            .max_depth(self.config.max_depth)
            .follow_links(self.config.follow_symlinks)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if path.is_file() && self.is_challenge_file(path) {
                match self.load_challenge_config(path) {
                    Ok(config) => {
                        println!("📁 Found challenge: {} ({})", config.name, config.category);
                        challenges.push(config);
                    }
                    Err(e) => {
                        eprintln!("❌ Failed to load challenge from {}: {}", path.display(), e);
                    }
                }
            }
        }

        if challenges.is_empty() {
            println!("ℹ️  No challenge files found. Supported patterns:");
            for pattern in &self.config.patterns {
                println!("  - {}", pattern);
            }
        }

        Ok(challenges)
    }

    /// Checks if a file is a challenge configuration file
    fn is_challenge_file(&self, path: &Path) -> bool {
        if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
            // Check exact filename patterns
            if self.config.patterns.iter().any(|p| filename == p) {
                return true;
            }

            // Check file extension
            if let Some(extension) = path.extension().and_then(|ext| ext.to_str()) {
                if self.config.extensions.iter().any(|ext| extension == ext) {
                    // Also check if filename contains "challenge"
                    return filename.to_lowercase().contains("challenge");
                }
            }
        }
        false
    }

    /// Loads a challenge configuration from file
    fn load_challenge_config(&self, path: &Path) -> Result<Challenge> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read {}", path.display()))?;

        let config = if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
            serde_json::from_str(&content)
                .with_context(|| format!("Failed to parse JSON from {}", path.display()))?
        } else {
            serde_yaml::from_str(&content)
                .with_context(|| format!("Failed to parse YAML from {}", path.display()))?
        };

        Ok(config)
    }

    /// Gets the relative path of a challenge file from base directory
    pub fn get_relative_path(&self, base_path: &Path, file_path: &Path) -> Result<PathBuf> {
        file_path
            .strip_prefix(base_path)
            .map(|p| p.to_path_buf())
            .map_err(|e| anyhow!("Failed to get relative path: {}", e))
    }

    /// Validates that all required files for a challenge exist
    pub fn validate_challenge_files(&self, config: &Challenge, base_path: &Path) -> Result<()> {
        // Check if files referenced in challenge exist
        if let Some(files) = &config.files {
            for file in files {
                let file_path = base_path.join(file.clone());
                if !file_path.exists() {
                    return Err(anyhow!(
                        "Referenced file does not exist: {}",
                        file_path.display()
                    ));
                }
            }
        }

        Ok(())
    }

    /// Finds all challenge files in a directory
    pub fn find_challenge_files(&self, base_path: &Path) -> Result<Vec<PathBuf>> {
        let mut files = Vec::new();

        for entry in WalkDir::new(base_path)
            .max_depth(self.config.max_depth)
            .follow_links(self.config.follow_symlinks)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if path.is_file() && self.is_challenge_file(path) {
                files.push(path.to_path_buf());
            }
        }

        Ok(files)
    }

    /// Gets statistics about scanned challenges
    pub fn get_stats(&self, challenges: &[Challenge]) -> ChallengeStats {
        let mut stats = ChallengeStats::default();

        for challenge in challenges {
            stats.total_challenges += 1;
            stats.total_points += challenge.value;

            // Fix: flags may be a private field or method, so count flags via public API or field
            // Assuming Challenge has a public 'flags' field that is a Vec or similar
            // If not, adjust accordingly
            stats.total_flags += challenge.flags.iter().count();

            if let Some(hints) = &challenge.hints {
                stats.total_hints += hints.len();
            }

            if let Some(files) = &challenge.files {
                stats.total_files += files.len();
            }

            // Track categories
            if !stats.categories.contains(&challenge.category) {
                stats.categories.push(challenge.category.clone());
            }
        }

        stats
    }
}

/// Statistics about scanned challenges
#[derive(Debug, Default, Serialize)]
pub struct ChallengeStats {
    pub total_challenges: usize,
    pub total_points: u32,
    pub total_flags: usize,
    pub total_hints: usize,
    pub total_files: usize,
    pub categories: Vec<String>,
}

impl ChallengeStats {
    /// Prints statistics in a human-readable format
    pub fn print(&self) {
        println!("📊 Challenge Statistics:");
        println!("  Total Challenges: {}", self.total_challenges);
        println!("  Total Points: {}", self.total_points);
        println!("  Total Flags: {}", self.total_flags);
        println!("  Total Hints: {}", self.total_hints);
        println!("  Total Files: {}", self.total_files);
        println!("  Categories: {}", self.categories.join(", "));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_is_challenge_file() {
        let scanner = DirectoryScanner::new();

        assert!(scanner.is_challenge_file(Path::new("challenge.yml")));
        assert!(scanner.is_challenge_file(Path::new("challenge.yaml")));
        assert!(scanner.is_challenge_file(Path::new("challenge.json")));
        assert!(scanner.is_challenge_file(Path::new("web-challenge.yml")));
        assert!(!scanner.is_challenge_file(Path::new("config.yml")));
        assert!(!scanner.is_challenge_file(Path::new("README.md")));
    }

    #[test]
    fn test_scan_empty_directory() -> Result<()> {
        let temp_dir = tempdir()?;
        let scanner = DirectoryScanner::new();

        let challenges = scanner.scan_directory(temp_dir.path())?;
        assert_eq!(challenges.len(), 0);

        Ok(())
    }
}
