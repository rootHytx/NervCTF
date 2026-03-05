//! CTFd Tag API endpoints
//! Handles all tag-related API operations

use super::super::models::Tag;
use crate::ctfd_api::client::CtfdClient;
use anyhow::Result;
use reqwest::Method;
use serde_json::Value;

impl CtfdClient {
    /// Get all tags
    pub async fn get_tags(&self) -> Result<Option<Vec<Tag>>> {
        self.execute(Method::GET, "/tags", None::<&()>).await
    }

    /// Get a specific tag by ID
    pub async fn get_tag(&self, id: u32) -> Result<Option<Tag>> {
        self.execute(Method::GET, &format!("/tags/{}", id), None::<&()>)
            .await
    }

    /// Create a new tag
    pub async fn create_tag(&self, data: &Value) -> Result<Option<Tag>> {
        self.execute(Method::POST, "/tags", Some(data)).await
    }

    /// Delete a tag
    pub async fn delete_tag(&self, id: u32) -> Result<()> {
        self.request_without_body(Method::DELETE, &format!("/tags/{}", id), None::<&()>)
            .await
    }
}
