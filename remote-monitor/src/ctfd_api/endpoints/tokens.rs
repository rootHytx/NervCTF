use crate::ctfd_api::CtfdClient;
use crate::ctfd_api::models::tokens::Token;
use anyhow::Result;
use reqwest::Method;
use serde_json::Value;

impl CtfdClient {
    /// POST /tokens - Create a new API token
    pub async fn create_token(&self, token_data: &Value) -> Result<Token> {
        self.execute(Method::POST, "/tokens", Some(token_data))
            .await
    }

    /// GET /tokens - List all API tokens
    pub async fn get_tokens(&self) -> Result<Vec<Token>> {
        self.execute(Method::GET, "/tokens", None::<&()>).await
    }

    /// GET /tokens/{token_id} - Get a specific API token
    pub async fn get_token(&self, token_id: u32) -> Result<Token> {
        self.execute(Method::GET, &format!("/tokens/{}", token_id), None::<&()>)
            .await
    }

    /// DELETE /tokens/{token_id} - Delete an API token
    pub async fn delete_token(&self, token_id: u32) -> Result<()> {
        self.request(
            Method::DELETE,
            &format!("/tokens/{}", token_id),
            None::<&()>,
        )
        .await?;
        Ok(())
    }

    /// GET /tokens/me - Get the current user's tokens
    pub async fn get_my_tokens(&self) -> Result<Vec<Token>> {
        self.execute(Method::GET, "/tokens/me", None::<&()>).await
    }
}
