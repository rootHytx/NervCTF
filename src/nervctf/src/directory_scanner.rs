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

/// A challenge file that could not be loaded during a directory scan.
#[derive(Debug)]
pub struct ScanFailure {
    pub path: PathBuf,
    pub error: String,
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

    /// Scans a directory for challenge files (backwards-compatible wrapper).
    /// Prints per-challenge progress; failures are printed to stderr.
    pub fn scan_directory(&self, base_path: &Path) -> Result<Vec<Challenge>> {
        let (challenges, failures) = self.scan_directory_full(base_path, true)?;
        for f in &failures {
            eprintln!(
                "❌ Failed to load challenge from {}: {}",
                f.path.display(),
                f.error
            );
        }
        Ok(challenges)
    }

    /// Scans a directory and returns both loaded challenges and parse failures.
    ///
    /// When `verbose` is true, prints `📁 Found challenge:` for every success.
    /// The `🔍 Scanning directory:` header is always printed.
    pub fn scan_directory_full(
        &self,
        base_path: &Path,
        verbose: bool,
    ) -> Result<(Vec<Challenge>, Vec<ScanFailure>)> {
        let mut challenges = Vec::new();
        let mut failures: Vec<ScanFailure> = Vec::new();

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
                        if verbose {
                            println!(
                                "📁 Found challenge: {} ({})",
                                config.name, config.category
                            );
                        }
                        challenges.push(config);
                    }
                    Err(e) => {
                        // Use {:#} to include the full error chain (root cause
                        // from serde_yaml includes line/column information).
                        failures.push(ScanFailure {
                            path: path.to_path_buf(),
                            error: format!("{:#}", e),
                        });
                    }
                }
            }
        }

        if verbose && challenges.is_empty() && failures.is_empty() {
            println!("ℹ️  No challenge files found. Supported patterns:");
            for pattern in &self.config.patterns {
                println!("  - {}", pattern);
            }
        }

        Ok((challenges, failures))
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

        let mut config: Challenge = if path.extension().and_then(|ext| ext.to_str()) == Some("json")
        {
            serde_json::from_str(&content)
                .with_context(|| format!("Failed to parse JSON from {}", path.display()))?
        } else {
            serde_yaml::from_str(&content)
                .with_context(|| format!("Failed to parse YAML from {}", path.display()))?
        };

        // Set source_path to the directory containing challenge.yml so that
        // relative file references in `files:` resolve correctly at deploy time.
        config.source_path = path
            .parent()
            .unwrap_or(Path::new("."))
            .to_string_lossy()
            .to_string();

        // Collect top-level YAML keys not recognised by the ctfcli spec so
        // the validator can warn about them.  JSON files are skipped since
        // they come from API responses, not from challenge authors.
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            const KNOWN_SPEC_KEYS: &[&str] = &[
                "name", "author", "category", "description", "attribution",
                "value", "type", "extra", "image", "protocol", "host",
                "connection_info", "healthcheck", "attempts", "flags",
                "topics", "tags", "files", "hints", "requirements", "next",
                "state", "version", "id", "challenge_id",
            ];
            if let Ok(serde_yaml::Value::Mapping(map)) =
                serde_yaml::from_str::<serde_yaml::Value>(&content)
            {
                for key in map.keys() {
                    if let serde_yaml::Value::String(k) = key {
                        if !KNOWN_SPEC_KEYS.contains(&k.as_str()) {
                            config.unknown_yaml_keys.push(k.clone());
                        }
                    }
                }
            }
        }

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

            stats.total_flags += challenge.flags.as_deref().unwrap_or(&[]).len();

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
    use std::fs;
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

    #[test]
    fn test_scan_finds_challenge_yml() -> Result<()> {
        let temp_dir = tempdir()?;
        let chall_dir = temp_dir.path().join("web/sqli");
        fs::create_dir_all(&chall_dir)?;
        fs::write(
            chall_dir.join("challenge.yml"),
            "name: SQL Injection\ncategory: Web\nvalue: 100\ntype: standard\nflags:\n  - flag{sql}\n",
        )?;

        let scanner = DirectoryScanner::new();
        let challenges = scanner.scan_directory(temp_dir.path())?;

        assert_eq!(challenges.len(), 1);
        assert_eq!(challenges[0].name, "SQL Injection");
        assert_eq!(challenges[0].category, "Web");
        Ok(())
    }

    #[test]
    fn test_scan_sets_source_path() -> Result<()> {
        let temp_dir = tempdir()?;
        let chall_dir = temp_dir.path().join("rev/easy");
        fs::create_dir_all(&chall_dir)?;
        fs::write(
            chall_dir.join("challenge.yml"),
            "name: Easy Rev\ncategory: Rev\nvalue: 50\ntype: standard\nflags:\n  - flag{rev}\n",
        )?;

        let scanner = DirectoryScanner::new();
        let challenges = scanner.scan_directory(temp_dir.path())?;

        assert_eq!(challenges.len(), 1);
        // source_path must point to the directory containing challenge.yml
        let expected = chall_dir.to_string_lossy().to_string();
        assert_eq!(challenges[0].source_path, expected);
        Ok(())
    }

    #[test]
    fn test_scan_multiple_challenges() -> Result<()> {
        let temp_dir = tempdir()?;
        for (cat, name, flag) in &[
            ("web", "XSS", "flag{xss}"),
            ("crypto", "Caesar", "flag{caesar}"),
            ("pwn", "Overflow", "flag{overflow}"),
        ] {
            let dir = temp_dir.path().join(cat);
            fs::create_dir_all(&dir)?;
            fs::write(
                dir.join("challenge.yml"),
                format!(
                    "name: {}\ncategory: {}\nvalue: 100\ntype: standard\nflags:\n  - {}\n",
                    name, cat, flag
                ),
            )?;
        }

        let scanner = DirectoryScanner::new();
        let challenges = scanner.scan_directory(temp_dir.path())?;
        assert_eq!(challenges.len(), 3);
        Ok(())
    }

    #[test]
    fn test_scan_skips_invalid_yaml() -> Result<()> {
        let temp_dir = tempdir()?;
        fs::write(temp_dir.path().join("challenge.yml"), "not: valid: yaml: {{{")?;

        let scanner = DirectoryScanner::new();
        // Should not panic or return an error — bad file is skipped with eprintln
        let challenges = scanner.scan_directory(temp_dir.path())?;
        assert_eq!(challenges.len(), 0);
        Ok(())
    }

    #[test]
    fn test_scan_nonexistent_directory() {
        let scanner = DirectoryScanner::new();
        let result = scanner.scan_directory(Path::new("/nonexistent/path/xyz"));
        assert!(result.is_err());
    }

    #[test]
    fn test_scan_file_path_errors() {
        let scanner = DirectoryScanner::new();
        // Passing a file instead of directory should return an error
        let result = scanner.scan_directory(Path::new("/etc/hostname"));
        assert!(result.is_err());
    }

    #[test]
    fn test_find_challenge_files_returns_paths() -> Result<()> {
        let temp_dir = tempdir()?;
        let sub = temp_dir.path().join("chall");
        fs::create_dir_all(&sub)?;
        fs::write(
            sub.join("challenge.yml"),
            "name: x\ncategory: y\nvalue: 1\ntype: standard\nflags:\n  - flag{x}\n",
        )?;

        let scanner = DirectoryScanner::new();
        let files = scanner.find_challenge_files(temp_dir.path())?;

        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("challenge.yml"));
        Ok(())
    }

    #[test]
    fn test_validate_challenge_files_missing() -> Result<()> {
        let temp_dir = tempdir()?;
        let yaml = "name: x\ncategory: y\nvalue: 1\ntype: standard\nflags:\n  - flag{x}\nfiles:\n  - missing.zip\n";
        let chall: Challenge = serde_yaml::from_str(yaml)?;

        let scanner = DirectoryScanner::new();
        let result = scanner.validate_challenge_files(&chall, temp_dir.path());
        assert!(result.is_err());
        Ok(())
    }

    #[test]
    fn test_validate_challenge_files_present() -> Result<()> {
        let temp_dir = tempdir()?;
        fs::write(temp_dir.path().join("binary"), b"ELF")?;

        let yaml = "name: x\ncategory: y\nvalue: 1\ntype: standard\nflags:\n  - flag{x}\nfiles:\n  - binary\n";
        let chall: Challenge = serde_yaml::from_str(yaml)?;

        let scanner = DirectoryScanner::new();
        let result = scanner.validate_challenge_files(&chall, temp_dir.path());
        assert!(result.is_ok());
        Ok(())
    }

    #[test]
    fn test_get_stats_counts_correctly() {
        let yaml = "name: x\ncategory: web\nvalue: 100\ntype: standard\nflags:\n  - flag{x}\n  - flag{alt}\nhints:\n  - free hint\n  - content: paid\n    cost: 50\nfiles:\n  - file.zip\n";
        let chall: Challenge = serde_yaml::from_str(yaml).unwrap();

        let scanner = DirectoryScanner::new();
        let stats = scanner.get_stats(&[chall]);

        assert_eq!(stats.total_challenges, 1);
        assert_eq!(stats.total_points, 100);
        assert_eq!(stats.total_flags, 2);
        assert_eq!(stats.total_hints, 2);
        assert_eq!(stats.total_files, 1);
        assert_eq!(stats.categories, vec!["web"]);
    }

    #[test]
    fn test_get_stats_multiple_categories() {
        let yamls = [
            "name: a\ncategory: web\nvalue: 100\ntype: standard\nflags:\n  - flag{a}\n",
            "name: b\ncategory: crypto\nvalue: 200\ntype: standard\nflags:\n  - flag{b}\n",
            "name: c\ncategory: web\nvalue: 50\ntype: standard\nflags:\n  - flag{c}\n",
        ];
        let challenges: Vec<Challenge> = yamls
            .iter()
            .map(|y| serde_yaml::from_str(y).unwrap())
            .collect();

        let scanner = DirectoryScanner::new();
        let stats = scanner.get_stats(&challenges);

        assert_eq!(stats.total_challenges, 3);
        assert_eq!(stats.total_points, 350);
        assert_eq!(stats.categories.len(), 2);
        assert!(stats.categories.contains(&"web".to_string()));
        assert!(stats.categories.contains(&"crypto".to_string()));
    }

    #[test]
    fn test_get_relative_path() -> Result<()> {
        let scanner = DirectoryScanner::new();
        let base = Path::new("/challenges");
        let target = Path::new("/challenges/web/sqli/challenge.yml");

        let rel = scanner.get_relative_path(base, target)?;
        assert_eq!(rel, Path::new("web/sqli/challenge.yml"));
        Ok(())
    }

    #[test]
    fn test_get_relative_path_not_under_base() {
        let scanner = DirectoryScanner::new();
        let base = Path::new("/challenges");
        let target = Path::new("/other/file.yml");

        let result = scanner.get_relative_path(base, target);
        assert!(result.is_err());
    }
}
