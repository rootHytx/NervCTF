use crate::ctfd_api::models::flags::Flag;
use crate::ctfd_api::CtfdClient;
use anyhow::Result;
use reqwest::Method;
use serde_json::Value;

impl CtfdClient {
    /// GET /flags - List all flags
    pub async fn get_flags(&self) -> Result<Vec<Flag>> {
        self.execute(Method::GET, "/flags", None::<&()>).await
    }

    /// GET /flags/{flag_id} - Get a specific flag
    pub async fn get_flag(&self, flag_id: u32) -> Result<Flag> {
        self.execute(Method::GET, &format!("/flags/{}", flag_id), None::<&()>)
            .await
    }

    /// POST /flags - Create a new flag
    pub async fn create_flag(&self, flag_data: &Flag) -> Result<Flag> {
        self.execute(Method::POST, "/flags", Some(flag_data)).await
    }

    /// PATCH /flags/{flag_id} - Update a flag
    pub async fn update_flag(&self, flag_id: u32, update_data: &Value) -> Result<Flag> {
        self.execute(
            Method::PATCH,
            &format!("/flags/{}", flag_id),
            Some(update_data),
        )
        .await
    }

    /// DELETE /flags/{flag_id} - Delete a flag
    pub async fn delete_flag(&self, flag_id: u32) -> Result<()> {
        self.request(Method::DELETE, &format!("/flags/{}", flag_id), None::<&()>)
            .await?;
        Ok(())
    }
}
