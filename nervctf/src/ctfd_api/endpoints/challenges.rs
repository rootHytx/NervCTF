//! CTFd Challenge API endpoints
//! Handles all challenge-related API operations

use super::super::models::Challenge;
use crate::ctfd_api::client::CtfdClient;
use anyhow::Result;
use reqwest::Method;
use serde_json::Value;

impl CtfdClient {
    /// Get all challenges
    pub async fn get_challenges(&self) -> Result<Option<Vec<Challenge>>> {
        self.execute(Method::GET, "/challenges", None::<&()>).await
    }

    /// Get a specific challenge by ID
    pub async fn get_challenge(&self, id: u32) -> Result<Option<Challenge>> {
        self.execute(Method::GET, &format!("/challenges/{}", id), None::<&()>)
            .await
    }
    /// Get a specific challenge ID by name
    pub async fn get_challenge_id(&self, name: &str) -> Result<Option<u32>> {
        if let Some(challenges) = self.get_challenges().await? {
            for challenge in challenges {
                if challenge.name == name {
                    return Ok(Option::from(challenge.id));
                }
            }
        }
        Ok(None)
    }

    /// Create a new challenge
    pub async fn create_challenge(&self, data: &Value) -> Result<Option<Challenge>> {
        self.execute(Method::POST, "/challenges", Some(data)).await
    }

    /// Update a challenge
    pub async fn update_challenge(&self, id: u32, data: &Value) -> Result<Option<Challenge>> {
        self.execute(Method::PATCH, &format!("/challenges/{}", id), Some(data))
            .await
    }

    /// Delete a challenge
    pub async fn delete_challenge(&self, id: u32) -> Result<()> {
        self.request_without_body(Method::DELETE, &format!("/challenges/{}", id), None::<&()>)
            .await
    }

    /// Get challenge files
    pub async fn get_challenge_files_endpoint(&self, id: u32) -> Result<Option<Value>> {
        self.execute(
            Method::GET,
            &format!("/challenges/{}/files", id),
            None::<&()>,
        )
        .await
    }

    /// Get challenge flags
    pub async fn get_challenge_flags_endpoint(&self, id: u32) -> Result<Option<Value>> {
        self.execute(
            Method::GET,
            &format!("/challenges/{}/flags", id),
            None::<&()>,
        )
        .await
    }

    /// Get challenge tags
    pub async fn get_challenge_tags_endpoint(&self, id: u32) -> Result<Option<Value>> {
        self.execute(
            Method::GET,
            &format!("/challenges/{}/tags", id),
            None::<&()>,
        )
        .await
    }

    /// Get challenge hints
    pub async fn get_challenge_hints_endpoint(&self, id: u32) -> Result<Option<Value>> {
        self.execute(
            Method::GET,
            &format!("/challenges/{}/hints", id),
            None::<&()>,
        )
        .await
    }
}
