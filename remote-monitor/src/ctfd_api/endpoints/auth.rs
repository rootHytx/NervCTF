use crate::ctfd_api::models::auth::{LoginRequest, LoginResponse};
use crate::ctfd_api::CtfdClient;
use anyhow::Result;
use reqwest::Method;
use serde_json::Value;

impl CtfdClient {
    /// POST /auth/login
    pub async fn login(&self, credentials: &LoginRequest) -> Result<LoginResponse> {
        self.execute(Method::POST, "/auth/login", Some(credentials))
            .await
    }

    /// POST /auth/register
    pub async fn register(&self, user_data: &Value) -> Result<()> {
        self.request(Method::POST, "/auth/register", Some(user_data))
            .await?;
        Ok(())
    }

    /// POST /auth/confirm
    pub async fn confirm_email(&self, confirmation_data: &Value) -> Result<()> {
        self.request(Method::POST, "/auth/confirm", Some(confirmation_data))
            .await?;
        Ok(())
    }

    /// POST /auth/reset_password
    pub async fn reset_password(&self, reset_data: &Value) -> Result<()> {
        self.request(Method::POST, "/auth/reset_password", Some(reset_data))
            .await?;
        Ok(())
    }

    /// GET /auth/oauth
    pub async fn get_oauth_providers(&self) -> Result<Value> {
        self.execute(Method::GET, "/auth/oauth", None::<&()>).await
    }
}
