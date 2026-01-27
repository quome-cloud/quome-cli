use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use reqwest::StatusCode;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::time::Duration;

use crate::api::models::ApiErrorResponse;
use crate::errors::{QuomeError, Result};
use crate::settings::Settings;

const USER_AGENT: &str = concat!("quome-cli/", env!("CARGO_PKG_VERSION"));

pub struct QuomeClient {
    http: reqwest::Client,
    base_url: String,
}

impl QuomeClient {
    pub fn new(token: Option<&str>, base_url: Option<&str>) -> Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        if let Some(t) = token {
            let auth_value = format!("Bearer {}", t);
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&auth_value).map_err(|_| QuomeError::InvalidResponse)?,
            );
        }

        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .default_headers(headers)
            .timeout(Duration::from_secs(30))
            .build()?;

        // Load settings and determine base URL
        let settings = Settings::load().unwrap_or_default();
        let base_url = base_url
            .map(String::from)
            .unwrap_or_else(|| settings.get_api_url());

        Ok(Self { http, base_url })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    async fn handle_response<T: DeserializeOwned>(&self, response: reqwest::Response) -> Result<T> {
        let status = response.status();

        if status.is_success() {
            let body = response.json::<T>().await?;
            Ok(body)
        } else {
            match status {
                StatusCode::UNAUTHORIZED => Err(QuomeError::Unauthorized),
                StatusCode::NOT_FOUND => {
                    let err = response.json::<ApiErrorResponse>().await.ok();
                    Err(QuomeError::NotFound(
                        err.map(|e| e.message)
                            .unwrap_or_else(|| "Resource not found".into()),
                    ))
                }
                StatusCode::TOO_MANY_REQUESTS => Err(QuomeError::RateLimited),
                _ => {
                    let err = response.json::<ApiErrorResponse>().await.ok();
                    Err(QuomeError::ApiError(err.map(|e| e.message).unwrap_or_else(
                        || format!("Request failed with status {}", status),
                    )))
                }
            }
        }
    }

    async fn handle_empty_response(&self, response: reqwest::Response) -> Result<()> {
        let status = response.status();

        if status.is_success() {
            Ok(())
        } else {
            match status {
                StatusCode::UNAUTHORIZED => Err(QuomeError::Unauthorized),
                StatusCode::NOT_FOUND => {
                    let err = response.json::<ApiErrorResponse>().await.ok();
                    Err(QuomeError::NotFound(
                        err.map(|e| e.message)
                            .unwrap_or_else(|| "Resource not found".into()),
                    ))
                }
                StatusCode::TOO_MANY_REQUESTS => Err(QuomeError::RateLimited),
                _ => {
                    let err = response.json::<ApiErrorResponse>().await.ok();
                    Err(QuomeError::ApiError(err.map(|e| e.message).unwrap_or_else(
                        || format!("Request failed with status {}", status),
                    )))
                }
            }
        }
    }

    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let response = self.http.get(self.url(path)).send().await?;
        self.handle_response(response).await
    }

    pub async fn post<T: DeserializeOwned, B: Serialize>(&self, path: &str, body: &B) -> Result<T> {
        let response = self.http.post(self.url(path)).json(body).send().await?;
        self.handle_response(response).await
    }

    pub async fn put<T: DeserializeOwned, B: Serialize>(&self, path: &str, body: &B) -> Result<T> {
        let response = self.http.put(self.url(path)).json(body).send().await?;
        self.handle_response(response).await
    }

    pub async fn delete(&self, path: &str) -> Result<()> {
        let response = self.http.delete(self.url(path)).send().await?;
        self.handle_empty_response(response).await
    }
}
