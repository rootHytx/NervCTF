use anyhow::{anyhow, Ok, Result};
use reqwest::blocking::Client as BlockingClient;
use reqwest::{blocking::multipart, header, Client, Method, Response};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;
use std::time::Duration;

const BASE_PATH: &str = "/api/v1";
const DEFAULT_TIMEOUT: u64 = 10;

/// Main client for interacting with the CTFd API
#[derive(Clone)]
pub struct CtfdClient {
    client: Client,
    blocking_client: BlockingClient,
    base_url: String,
}

impl CtfdClient {
    /// Creates a new CTFd client instance
    ///
    /// # Arguments
    /// * `base_url` - Base URL of the CTFd instance (e.g., "https://ctfd.example.com")
    /// * `api_key` - API key for authentication
    pub fn new(base_url: &str, api_key: &str) -> Result<Self> {
        // Only set Authorization as a default header.
        // Content-Type is intentionally omitted here:
        //   - JSON requests set it via .json(body) in request()
        //   - Multipart requests set it via .multipart(form) in upload_file()
        // Having Content-Type: application/json as a default causes 500s on
        // CTFd's file upload endpoint.
        let mut headers = header::HeaderMap::new();
        headers.insert(
            "Authorization",
            header::HeaderValue::from_str(&format!("Token {}", api_key))
                .map_err(|e| anyhow!("Invalid API key: {}", e))?,
        );

        let client = Client::builder()
            .default_headers(headers.clone())
            .timeout(Duration::from_secs(DEFAULT_TIMEOUT))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        let blocking_client = BlockingClient::builder()
            .default_headers(headers)
            .timeout(Duration::from_secs(DEFAULT_TIMEOUT))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;

        Ok(Self {
            client,
            blocking_client,
            base_url: base_url.trim_end_matches('/').to_string(),
        })
    }

    /// Executes an API request
    pub async fn request<T: Serialize + ?Sized>(
        &self,
        method: Method,
        endpoint: &str,
        body: Option<&T>,
    ) -> Result<Response> {
        let url = format!("{}{}{}", self.base_url, BASE_PATH, endpoint);
        let mut builder = self.client.request(method.clone(), &url);

        if let Some(body) = body {
            builder = builder.json(body);
        }

        let response = builder.send().await?;
        let status = response.status();

        if status.is_success() {
            Ok(response)
        } else {
            let error_text = response.text().await?;
            Err(anyhow!(
                "API error ({} {}): {}",
                method,
                endpoint,
                error_text
            ))
        }
    }

    pub async fn parse_response<T: DeserializeOwned>(response: Response) -> Result<T> {
        let url = response.url().to_string();
        let status = response.status();
        let bytes = response.bytes().await?;
        if bytes.is_empty() {
            return Err(anyhow!(
                "Empty response body from {} (HTTP {})",
                url,
                status
            ));
        }
        let json: Value = serde_json::from_slice(&bytes).map_err(|e| {
            anyhow!(
                "JSON parse error from {} (HTTP {}): {}\nBody: {}",
                url,
                status,
                e,
                String::from_utf8_lossy(&bytes)
                    .chars()
                    .take(500)
                    .collect::<String>()
            )
        })?;
        if let Some(error) = json.get("error") {
            Err(anyhow!("API error: {}", error))
        } else if let Some(data) = json.get("data") {
            serde_json::from_value(data.clone())
                .map_err(|e| anyhow!("Deserialization error: {}", e))
        } else {
            // Fallback: try to deserialize the whole JSON value
            serde_json::from_value(json.clone()).map_err(|e| {
                anyhow!(
                    "Unexpected response format and deserialization error: {}",
                    e
                )
            })
        }
    }

    /// Executes a request and parses the response
    pub async fn execute<T: DeserializeOwned, B: Serialize + ?Sized>(
        &self,
        method: Method,
        endpoint: &str,
        body: Option<&B>,
    ) -> Result<Option<T>> {
        let response = self.request(method.clone(), endpoint, body).await?;
        if method != Method::DELETE {
            let parsed = Self::parse_response(response).await?;
            Ok(Some(parsed))
        } else {
            Ok(None)
        }
    }

    /// Executes a request and parses the response (multipart/form-data)
    /// NOTE: kept for legacy callers; prefer upload_file for async contexts.
    pub async fn post_file<T: DeserializeOwned>(
        &self,
        endpoint: &str,
        form: Option<multipart::Form>,
    ) -> Result<Option<T>> {
        let url = format!("{}{}{}", self.base_url, BASE_PATH, endpoint);
        if let Some(form) = form {
            self.blocking_client.post(&url).multipart(form).send()?;
        };
        Ok(None)
    }

    /// Upload a file using the async client (safe to call from within tokio).
    pub async fn upload_file(&self, endpoint: &str, form: reqwest::multipart::Form) -> Result<()> {
        let url = format!("{}{}{}", self.base_url, BASE_PATH, endpoint);
        let response = self
            .client
            .post(&url)
            .timeout(Duration::from_secs(120)) // allow 2 min for large files
            .header("Accept", "application/json")
            .multipart(form)
            .send()
            .await?;
        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            let error_text = response.text().await?;
            Err(anyhow!("File upload failed ({}): {}", status, error_text))
        }
    }

    /// Executes a request with query parameters and parses the response
    pub async fn execute_with_params<T: DeserializeOwned, B: Serialize + ?Sized, P: Serialize>(
        &self,
        method: Method,
        endpoint: &str,
        body: Option<&B>,
        params: &P,
    ) -> Result<T> {
        let url = format!("{}{}{}", self.base_url, BASE_PATH, endpoint);
        let mut builder = self.client.request(method.clone(), &url).query(params);

        if let Some(body) = body {
            builder = builder.json(body);
        }

        let response = builder.send().await?;
        let status = response.status();

        if status.is_success() {
            Self::parse_response(response).await
        } else {
            let error_text = response.text().await?;
            Err(anyhow!(
                "API error ({} {}): {}",
                method,
                endpoint,
                error_text
            ))
        }
    }

    /// Executes a request without expecting a response body
    pub async fn request_without_body<T: Serialize + ?Sized>(
        &self,
        method: Method,
        endpoint: &str,
        body: Option<&T>,
    ) -> Result<()> {
        self.request(method, endpoint, body).await?;
        Ok(())
    }
}
