use crate::ctfd_api::models::tags::Tag;
use crate::ctfd_api::CtfdClient;
use anyhow::Result;
use reqwest::Method;
use serde_json::Value;

impl CtfdClient {
    /// GET /tags - List all tags
    pub async fn get_tags(&self) -> Result<Vec<Tag>> {
        self.execute(Method::GET, "/tags", None::<&()>).await
    }

    /// GET /tags/{tag_id} - Get a specific tag
    pub async fn get_tag(&self, tag_id: u32) -> Result<Tag> {
        self.execute(Method::GET, &format!("/tags/{}", tag_id), None::<&()>)
            .await
    }

    /// POST /tags - Create a new tag
    pub async fn create_tag(&self, tag_data: &Value) -> Result<Tag> {
        self.execute(Method::POST, "/tags", Some(tag_data)).await
    }

    /// PATCH /tags/{tag_id} - Update a tag
    pub async fn update_tag(&self, tag_id: u32, update_data: &Value) -> Result<Tag> {
        self.execute(
            Method::PATCH,
            &format!("/tags/{}", tag_id),
            Some(update_data),
        )
        .await
    }

    /// DELETE /tags/{tag_id} - Delete a tag
    pub async fn delete_tag(&self, tag_id: u32) -> Result<()> {
        self.request(Method::DELETE, &format!("/tags/{}", tag_id), None::<&()>)
            .await?;
        Ok(())
    }
}
