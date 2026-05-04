use crate::ctfd_api::client::CtfdClient;
use anyhow::Result;
use reqwest::Method;

impl CtfdClient {
    pub async fn delete_flag(&self, id: u32) -> Result<()> {
        self.request_without_body(Method::DELETE, &format!("/flags/{}", id), None::<&()>)
            .await
    }
}
