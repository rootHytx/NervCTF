use crate::ctfd_api::models::challenges::Challenge;
use crate::ctfd_api::CtfdClient;
use anyhow::Result;
use reqwest::Method;
use serde_json::json;
use serde_json::Value;

impl CtfdClient {
    /// GET /challenges - List all challenges
    pub async fn get_challenges(&self) -> Result<Vec<Challenge>> {
        self.execute(Method::GET, "/challenges", None::<&()>).await
    }

    /// GET /challenges/{challenge_id} - Get a specific challenge
    pub async fn get_challenge(&self, challenge_id: u32) -> Result<Challenge> {
        self.execute(
            Method::GET,
            &format!("/challenges/{}", challenge_id),
            None::<&()>,
        )
        .await
    }

    /// POST /challenges - Create a new challenge
    pub async fn create_challenge(&self, challenge_data: &Value) -> Result<Challenge> {
        self.execute(Method::POST, "/challenges", Some(challenge_data))
            .await
    }

    /// PATCH /challenges/{challenge_id} - Update a challenge
    pub async fn update_challenge(
        &self,
        challenge_id: u32,
        update_data: &Value,
    ) -> Result<Challenge> {
        self.execute(
            Method::PATCH,
            &format!("/challenges/{}", challenge_id),
            Some(update_data),
        )
        .await
    }

    /// DELETE /challenges/{challenge_id} - Delete a challenge
    pub async fn delete_challenge(&self, challenge_id: u32) -> Result<()> {
        self.request(
            Method::DELETE,
            &format!("/challenges/{}", challenge_id),
            None::<&()>,
        )
        .await?;
        Ok(())
    }

    /// GET /challenges/types - Get available challenge types
    pub async fn get_challenge_types(&self) -> Result<Vec<String>> {
        self.execute(Method::GET, "/challenges/types", None::<&()>)
            .await
    }

    /// POST /challenges/attempt - Attempt to solve a challenge
    pub async fn attempt_challenge(&self, challenge_id: u32, submission: &str) -> Result<Value> {
        let body = json!({
            "challenge_id": challenge_id,
            "submission": submission
        });
        self.execute(Method::POST, "/challenges/attempt", Some(&body))
            .await
    }

    /// GET /challenges/{challenge_id}/solves - Get solves for a challenge
    pub async fn get_challenge_solves(&self, challenge_id: u32) -> Result<Value> {
        self.execute(
            Method::GET,
            &format!("/challenges/{}/solves", challenge_id),
            None::<&()>,
        )
        .await
    }

    /// GET /challenges/{challenge_id}/files - Get files for a challenge
    pub async fn get_challenge_files(&self, challenge_id: u32) -> Result<Value> {
        self.execute(
            Method::GET,
            &format!("/challenges/{}/files", challenge_id),
            None::<&()>,
        )
        .await
    }

    /// GET /challenges/{challenge_id}/flags - Get flags for a challenge
    pub async fn get_challenge_flags(&self, challenge_id: u32) -> Result<Value> {
        self.execute(
            Method::GET,
            &format!("/challenges/{}/flags", challenge_id),
            None::<&()>,
        )
        .await
    }

    /// GET /challenges/{challenge_id}/tags - Get tags for a challenge
    pub async fn get_challenge_tags(&self, challenge_id: u32) -> Result<Value> {
        self.execute(
            Method::GET,
            &format!("/challenges/{}/tags", challenge_id),
            None::<&()>,
        )
        .await
    }

    /// GET /challenges/{challenge_id}/hints - Get hints for a challenge
    pub async fn get_challenge_hints(&self, challenge_id: u32) -> Result<Value> {
        self.execute(
            Method::GET,
            &format!("/challenges/{}/hints", challenge_id),
            None::<&()>,
        )
        .await
    }
}
