//! CTFd Hint API endpoints
//! Handles all hint-related API operations

use super::super::models::Hint;
use crate::ctfd_api::client::CtfdClient;
use anyhow::Result;
use reqwest::Method;
use serde_json::Value;

impl CtfdClient {
    /// Get all hints
    pub async fn get_hints(&self) -> Result<Option<Vec<Hint>>> {
        self.execute(Method::GET, "/hints", None::<&()>).await
    }

    /// Get a specific hint by ID
    pub async fn get_hint(&self, id: u32) -> Result<Option<Hint>> {
        self.execute(Method::GET, &format!("/hints/{}", id), None::<&()>)
            .await
    }

    /// Create a new hint
    pub async fn create_hint(&self, data: &Value) -> Result<Option<Hint>> {
        self.execute(Method::POST, "/hints", Some(data)).await
    }

    /// Delete a hint
    pub async fn delete_hint(&self, id: u32) -> Result<()> {
        self.request_without_body(Method::DELETE, &format!("/hints/{}", id), None::<&()>)
            .await
    }
}
