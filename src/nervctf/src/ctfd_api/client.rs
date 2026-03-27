use anyhow::{anyhow, Ok, Result};
use reqwest::{header, Client, Method, Response};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;
use std::time::Duration;

const BASE_PATH: &str = "/api/v1";
const DEFAULT_TIMEOUT: u64 = 10;

#[derive(Clone)]
pub struct CtfdClient {
    client: Client,
    base_url: String,
}

impl CtfdClient {
    pub fn new(monitor_url: &str, monitor_token: &str) -> Result<Self> {
        let mut headers = header::HeaderMap::new();
        headers.insert(
            "Authorization",
            header::HeaderValue::from_str(&format!("Token {}", monitor_token))
                .map_err(|e| anyhow!("Invalid monitor token: {}", e))?,
        );
        headers.insert(
            "Accept",
            header::HeaderValue::from_static("application/json"),
        );

        let client = Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_secs(DEFAULT_TIMEOUT))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;

        Ok(Self {
            client,
            base_url: monitor_url.trim_end_matches('/').to_string(),
        })
    }

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

    async fn parse_response<T: DeserializeOwned>(response: Response) -> Result<T> {
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
            serde_json::from_value(json.clone()).map_err(|e| {
                anyhow!(
                    "Unexpected response format and deserialization error: {}",
                    e
                )
            })
        }
    }

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

    /// Upload a file using the async client (safe to call from within tokio).
    pub async fn upload_file(&self, endpoint: &str, form: reqwest::multipart::Form) -> Result<()> {
        let url = format!("{}{}{}", self.base_url, BASE_PATH, endpoint);
        let response = self
            .client
            .post(&url)
            .timeout(Duration::from_secs(120))
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
