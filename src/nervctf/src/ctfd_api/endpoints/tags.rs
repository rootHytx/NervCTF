use super::super::models::Tag;
use crate::ctfd_api::client::CtfdClient;
use anyhow::Result;
use reqwest::Method;
use serde_json::Value;

impl CtfdClient {
    pub async fn get_tags(&self) -> Result<Option<Vec<Tag>>> {
        self.execute(Method::GET, "/tags", None::<&()>).await
    }

    pub async fn get_tag(&self, id: u32) -> Result<Option<Tag>> {
        self.execute(Method::GET, &format!("/tags/{}", id), None::<&()>)
            .await
    }

    pub async fn create_tag(&self, data: &Value) -> Result<Option<Tag>> {
        self.execute(Method::POST, "/tags", Some(data)).await
    }

    pub async fn delete_tag(&self, id: u32) -> Result<()> {
        self.request_without_body(Method::DELETE, &format!("/tags/{}", id), None::<&()>)
            .await
    }
}
