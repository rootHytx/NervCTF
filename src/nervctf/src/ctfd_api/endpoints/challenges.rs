//! CTFd Challenge API endpoints
//! Handles all challenge-related API operations

use super::super::models::Challenge;
use crate::ctfd_api::client::CtfdClient;
use anyhow::Result;
use reqwest::Method;
use serde_json::Value;

impl CtfdClient {
    /// Get all challenges, following CTFd pagination automatically.
    ///
    /// CTFd paginates `/api/v1/challenges` (default 20 per page).  Without
    /// explicit pagination we would only see the first page, causing challenges
    /// beyond page 1 to appear as "new" on every re-deploy.
    pub async fn get_challenges(&self) -> Result<Option<Vec<Challenge>>> {
        let mut all: Vec<Challenge> = Vec::new();
        let mut page: u64 = 1;

        loop {
            let endpoint = format!("/challenges?page={}", page);
            let response = self.request(Method::GET, &endpoint, None::<&()>).await?;
            let bytes = response.bytes().await?;
            let json: Value = serde_json::from_slice(&bytes)
                .map_err(|e| anyhow::anyhow!("JSON parse error on {}: {}", endpoint, e))?;

            // Collect this page's challenges
            if let Some(arr) = json.get("data").and_then(|d| d.as_array()) {
                if arr.is_empty() {
                    break;
                }
                let page_challenges: Vec<Challenge> =
                    serde_json::from_value(Value::Array(arr.clone()))
                        .map_err(|e| anyhow::anyhow!("Deserialize challenges page {}: {}", page, e))?;
                all.extend(page_challenges);
            } else {
                break;
            }

            // Advance to next page if CTFd says there is one
            let next = json
                .pointer("/meta/pagination/next")
                .and_then(|v| v.as_u64());
            match next {
                Some(n) if n != page => page = n,
                _ => break,
            }
        }

        if all.is_empty() { Ok(None) } else { Ok(Some(all)) }
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
