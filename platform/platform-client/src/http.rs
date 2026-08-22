//! The HTTP transport: `POST /graphql` with a bearer token (G.3).

use crate::{Error, Transport};

/// A client of one platform's `/graphql` as one API token.
#[derive(Clone)]
pub struct Http {
    client: reqwest::Client,
    url: String,
    token: String,
}

impl Http {
    /// `url` is the full `/graphql` endpoint; `token` the `varve_…`
    /// secret, sent as `Authorization: Bearer`.
    pub fn new(url: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            url: url.into(),
            token: token.into(),
        }
    }
}

impl Transport for Http {
    async fn execute(&self, request: serde_json::Value) -> Result<serde_json::Value, Error> {
        let response = self
            .client
            .post(&self.url)
            .bearer_auth(&self.token)
            .json(&request)
            .send()
            .await
            .map_err(|e| Error::Transport(e.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            return Err(Error::Transport(format!("{}: {status}", self.url)));
        }
        response
            .json()
            .await
            .map_err(|e| Error::Transport(e.to_string()))
    }
}
