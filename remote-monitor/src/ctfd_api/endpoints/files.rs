use crate::ctfd_api::models::files::File;
use crate::ctfd_api::CtfdClient;
use anyhow::{anyhow, Result};
use reqwest::Method;
use std::path::Path;

impl CtfdClient {
    /// GET /files - List all files
    pub async fn get_files(&self) -> Result<Vec<File>> {
        self.execute(Method::GET, "/files", None::<&()>).await
    }

    /// GET /files/{file_id} - Get a specific file
    pub async fn get_file(&self, file_id: u32) -> Result<File> {
        self.execute(Method::GET, &format!("/files/{}", file_id), None::<&()>)
            .await
    }

    /// POST /files - Upload a new file
    pub async fn upload_file<P: AsRef<Path>>(&self, file_path: P) -> Result<File> {
        // This will require multipart form data handling
        // Implementation will be added once we enhance the client
        Err(anyhow!(
            "File upload not implemented yet. Requires multipart support in client."
        ))
    }

    /// DELETE /files/{file_id} - Delete a file
    pub async fn delete_file(&self, file_id: u32) -> Result<()> {
        self.request(Method::DELETE, &format!("/files/{}", file_id), None::<&()>)
            .await?;
        Ok(())
    }
}
