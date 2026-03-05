//! CTFd FlagContent API endpoints
//! Handles all flag-related API operations

use super::super::models::FlagContent;
use crate::ctfd_api::client::CtfdClient;
use anyhow::Result;
use reqwest::Method;
use serde_json::Value;

impl CtfdClient {
    /// Get all flags
    pub async fn get_flags(&self) -> Result<Option<Vec<FlagContent>>> {
        self.execute(Method::GET, "/flags", None::<&()>).await
    }

    /// Get a specific flag by ID
    pub async fn get_flag(&self, id: u32) -> Result<Option<FlagContent>> {
        self.execute(Method::GET, &format!("/flags/{}", id), None::<&()>)
            .await
    }

    /// Create a new flag
    pub async fn create_flag(&self, data: &Value) -> Result<Option<FlagContent>> {
        self.execute(Method::POST, "/flags", Some(data)).await
    }

    /// Delete a flag
    pub async fn delete_flag(&self, id: u32) -> Result<()> {
        self.request_without_body(Method::DELETE, &format!("/flags/{}", id), None::<&()>)
            .await
    }
}
