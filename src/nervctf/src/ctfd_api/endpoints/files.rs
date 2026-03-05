//! CTFd File API endpoints
//! Handles all file-related API operations

use super::super::models::File;
use crate::ctfd_api::client::CtfdClient;
use anyhow::Result;
use reqwest::{blocking::multipart::Form, Method};

impl CtfdClient {
    /// Get all files
    pub async fn get_files(&self) -> Result<Option<Vec<File>>> {
        self.execute(Method::GET, "/files", None::<&()>).await
    }

    pub async fn create_file(&self, form: Form) -> Result<Option<File>> {
        self.post_file("/files", Some(form)).await
    }

    /// Get a specific file by ID
    pub async fn get_file(&self, id: u32) -> Result<Option<File>> {
        self.execute(Method::GET, &format!("/files/{}", id), None::<&()>)
            .await
    }
    /// Delete a file
    pub async fn delete_file(&self, id: u32) -> Result<()> {
        self.request_without_body(Method::DELETE, &format!("/files/{}", id), None::<&()>)
            .await
    }
}
