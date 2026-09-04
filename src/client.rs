use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
use reqwest::StatusCode;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::time::Duration;

use crate::api::models::PaginatedResponse;
use crate::errors::{QuomeError, Result};
use crate::settings::Settings;

/// Page size used by `list_all_pages` — matches the `?limit=100` already
/// hard-coded on the individual list endpoints.
const LIST_ALL_PAGE_SIZE: i64 = 100;

const USER_AGENT: &str = concat!("quome-cli/", env!("CARGO_PKG_VERSION"));

pub struct QuomeClient {
    http: reqwest::Client,
    base_url: String,
}

/// FastAPI error bodies are `{"detail": "..."}` where detail may also be a
/// structured object (validation errors). Extract something readable either way.
fn extract_detail(text: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    match value.get("detail") {
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(other) => Some(other.to_string()),
        None => value
            .get("message")
            .and_then(|m| m.as_str())
            .map(String::from),
    }
}

impl QuomeClient {
    pub fn new(token: Option<&str>, base_url: Option<&str>) -> Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        if let Some(t) = token {
            let mut key_value =
                HeaderValue::from_str(t).map_err(|_| QuomeError::InvalidResponse)?;
            key_value.set_sensitive(true);
            headers.insert("X-API-Key", key_value);
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

    async fn error_from_response(&self, response: reqwest::Response) -> QuomeError {
        let status = response.status();
        match status {
            StatusCode::UNAUTHORIZED => QuomeError::Unauthorized,
            StatusCode::NOT_FOUND => {
                let text = response.text().await.unwrap_or_default();
                QuomeError::NotFound(
                    extract_detail(&text).unwrap_or_else(|| "Resource not found".into()),
                )
            }
            StatusCode::TOO_MANY_REQUESTS => QuomeError::RateLimited,
            _ => {
                let text = response.text().await.unwrap_or_default();
                QuomeError::ApiError(
                    extract_detail(&text)
                        .unwrap_or_else(|| format!("Request failed with status {}", status)),
                )
            }
        }
    }

    async fn handle_response<T: DeserializeOwned>(&self, response: reqwest::Response) -> Result<T> {
        if response.status().is_success() {
            let text = response.text().await?;
            if std::env::var("QUOME_DEBUG").is_ok() {
                eprintln!("DEBUG response: {}", text);
            }
            let body: T = serde_json::from_str(&text)?;
            Ok(body)
        } else {
            Err(self.error_from_response(response).await)
        }
    }

    async fn handle_empty_response(&self, response: reqwest::Response) -> Result<()> {
        if response.status().is_success() {
            Ok(())
        } else {
            Err(self.error_from_response(response).await)
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

    #[allow(dead_code)]
    pub async fn patch<T: DeserializeOwned, B: Serialize>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T> {
        let response = self.http.patch(self.url(path)).json(body).send().await?;
        self.handle_response(response).await
    }

    pub async fn delete(&self, path: &str) -> Result<()> {
        let response = self.http.delete(self.url(path)).send().await?;
        self.handle_empty_response(response).await
    }

    /// Fetch every page of a `PaginatedResponse<T>` list endpoint, following
    /// `meta.has_more` with `limit`/`offset`. `base_path` must not already
    /// carry a `limit`/`offset` query param; it may or may not have other
    /// query params (`?` vs `&` is joined correctly either way).
    ///
    /// Page-one-only lookups false-negative in orgs with >100 rows — this is
    /// the port of the Python CLI's `has_more`-driven loop (`api.py::iter_apps`
    /// and friends).
    pub async fn list_all_pages<T: DeserializeOwned>(&self, base_path: &str) -> Result<Vec<T>> {
        let sep = if base_path.contains('?') { '&' } else { '?' };
        let mut all = Vec::new();
        let mut offset: i64 = 0;
        loop {
            let path = format!(
                "{}{}limit={}&offset={}",
                base_path, sep, LIST_ALL_PAGE_SIZE, offset
            );
            let page: PaginatedResponse<T> = self.get(&path).await?;
            let got = page.data.len();
            all.extend(page.data);
            let has_more = page.meta.and_then(|m| m.has_more).unwrap_or(false);
            if !has_more || got == 0 {
                break;
            }
            offset += got as i64;
        }
        Ok(all)
    }
}
