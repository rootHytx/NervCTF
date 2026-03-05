//! CTFd Configurator Library
//! Provides comprehensive CTFd challenge management functionality
//! Auto-detects challenges in current directory structure

pub mod challenge_manager;
pub mod ctfd_api;
pub mod directory_scanner;
pub mod fix;
pub mod setup;
pub mod utils;
pub mod validator;

// Re-export for convenience
pub use challenge_manager::{utils as challenge_utils, ChallengeManager};
pub use ctfd_api::CtfdClient;
pub use directory_scanner::{ChallengeStats, DirectoryScanner};
pub use utils::*;
