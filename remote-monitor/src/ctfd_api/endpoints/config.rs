use crate::ctfd_api::CtfdClient;
use anyhow::Result;
use reqwest::Method;
use serde_json::Value;

impl CtfdClient {
    /// GET /configs - Get all configuration settings
    pub async fn get_configs(&self) -> Result<Value> {
        self.execute(Method::GET, "/configs", None::<&()>).await
    }

    /// GET /configs/{key} - Get a specific configuration setting
    pub async fn get_config(&self, key: &str) -> Result<Value> {
        self.execute(Method::GET, &format!("/configs/{}", key), None::<&()>)
            .await
    }

    /// PATCH /configs - Update configuration settings
    pub async fn update_configs(&self, config_data: &Value) -> Result<Value> {
        self.execute(Method::PATCH, "/configs", Some(config_data))
            .await
    }

    /// GET /configs/smtp - Get SMTP configuration
    pub async fn get_smtp_config(&self) -> Result<Value> {
        self.execute(Method::GET, "/configs/smtp", None::<&()>)
            .await
    }

    /// PATCH /configs/smtp - Update SMTP configuration
    pub async fn update_smtp_config(&self, smtp_data: &Value) -> Result<Value> {
        self.execute(Method::PATCH, "/configs/smtp", Some(smtp_data))
            .await
    }

    /// GET /configs/theme - Get theme configuration
    pub async fn get_theme_config(&self) -> Result<Value> {
        self.execute(Method::GET, "/configs/theme", None::<&()>)
            .await
    }

    /// PATCH /configs/theme - Update theme configuration
    pub async fn update_theme_config(&self, theme_data: &Value) -> Result<Value> {
        self.execute(Method::PATCH, "/configs/theme", Some(theme_data))
            .await
    }

    /// GET /configs/backup - Get backup configuration
    pub async fn get_backup_config(&self) -> Result<Value> {
        self.execute(Method::GET, "/configs/backup", None::<&()>)
            .await
    }
}
