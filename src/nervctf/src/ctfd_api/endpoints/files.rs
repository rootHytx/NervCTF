use crate::ctfd_api::client::CtfdClient;
use anyhow::Result;
use reqwest::Method;

impl CtfdClient {
    pub async fn delete_file(&self, id: u32) -> Result<()> {
        self.request_without_body(Method::DELETE, &format!("/files/{}", id), None::<&()>)
            .await
    }
}
