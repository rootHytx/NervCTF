use crate::ctfd_api::models::hints::Hint;
use crate::ctfd_api::CtfdClient;
use anyhow::Result;
use reqwest::Method;
use serde_json::Value;

impl CtfdClient {
    /// GET /hints - List all hints
    pub async fn get_hints(&self) -> Result<Vec<Hint>> {
        self.execute(Method::GET, "/hints", None::<&()>).await
    }

    /// GET /hints/{hint_id} - Get a specific hint
    pub async fn get_hint(&self, hint_id: u32) -> Result<Hint> {
        self.execute(Method::GET, &format!("/hints/{}", hint_id), None::<&()>)
            .await
    }

    /// POST /hints - Create a new hint
    pub async fn create_hint(&self, hint_data: &Value) -> Result<Hint> {
        self.execute(Method::POST, "/hints", Some(hint_data)).await
    }

    /// PATCH /hints/{hint_id} - Update a hint
    pub async fn update_hint(&self, hint_id: u32, update_data: &Value) -> Result<Hint> {
        self.execute(
            Method::PATCH,
            &format!("/hints/{}", hint_id),
            Some(update_data),
        )
        .await
    }

    /// DELETE /hints/{hint_id} - Delete a hint
    pub async fn delete_hint(&self, hint_id: u32) -> Result<()> {
        self.request(Method::DELETE, &format!("/hints/{}", hint_id), None::<&()>)
            .await?;
        Ok(())
    }

    /// POST /hints/{hint_id}/unlock - Unlock a hint
    pub async fn unlock_hint(&self, hint_id: u32) -> Result<Value> {
        self.execute(
            Method::POST,
            &format!("/hints/{}/unlock", hint_id),
            None::<&()>,
        )
        .await
    }
}
