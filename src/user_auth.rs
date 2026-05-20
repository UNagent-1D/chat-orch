//! Thin client for User-Auth (UNagent-1D/User-Auth) microservice.
//!
//! Used by the Telegram path to enroll new users on first contact, deliver
//! OTP codes via email, and exchange a verified code for Tenant's canonical
//! session JWT.

use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::error::AppError;

#[derive(Clone)]
pub struct UserAuthClient {
    http: Client,
    base_url: String,
}

#[derive(Debug, Serialize)]
pub struct CreateUserBody<'a> {
    pub tenant_id: &'a str,
    pub tenant_slug: &'a str,
    pub user_name: &'a str,
    pub user_last_name: &'a str,
    pub user_document: &'a str,
    pub user_email: &'a str,
}

#[derive(Debug, Serialize)]
struct DocumentOnly<'a> {
    document: &'a str,
}

#[derive(Debug, Serialize)]
struct VerifyBody<'a> {
    document: &'a str,
    code: &'a str,
}

/// Subset of User-Auth's verify-code response (which is Tenant's
/// `/auth/login` shape). We just need the session token to confirm success
/// — the JWT itself is forwarded verbatim into the in-memory auth state.
#[derive(Debug, Deserialize)]
pub struct VerifyResponse {
    pub token: String,
    #[serde(default)]
    pub user: Option<UserInfo>,
}

#[derive(Debug, Deserialize)]
pub struct UserInfo {
    pub id: String,
    pub email: String,
    pub role: String,
    #[serde(default)]
    pub tenant_id: Option<String>,
}

impl UserAuthClient {
    pub fn new(http: Client, base_url: impl Into<String>) -> Self {
        Self {
            http,
            base_url: base_url.into().trim_end_matches('/').to_string(),
        }
    }

    /// Create a user_info row. Returns Ok even on 409/conflict so the
    /// Telegram flow can re-enroll an existing chat_id without erroring.
    pub async fn create_user(&self, body: &CreateUserBody<'_>) -> Result<(), AppError> {
        let resp = self
            .http
            .post(format!("{}/auth/users", self.base_url))
            .json(body)
            .send()
            .await
            .map_err(|e| AppError::Downstream(format!("user-auth create_user: {e}")))?;

        let status = resp.status();
        if status.is_success() || status.as_u16() == 409 {
            return Ok(());
        }
        // Existing-user collisions surface as 500 from User-Auth because the
        // underlying repository returns a generic "Failed to create user"
        // error. Treat any 5xx with the body containing "duplicate" or
        // "unique" as idempotent success.
        let body = resp.text().await.unwrap_or_default();
        if body.to_lowercase().contains("duplicate")
            || body.to_lowercase().contains("unique")
            || body.to_lowercase().contains("already exists")
        {
            return Ok(());
        }
        Err(AppError::Downstream(format!(
            "user-auth create_user: status {status}: {body}"
        )))
    }

    pub async fn request_code(&self, document: &str) -> Result<(), AppError> {
        let resp = self
            .http
            .post(format!("{}/auth/request-code", self.base_url))
            .json(&DocumentOnly { document })
            .send()
            .await
            .map_err(|e| AppError::Downstream(format!("user-auth request-code: {e}")))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            let s = resp.status();
            let b = resp.text().await.unwrap_or_default();
            Err(AppError::Downstream(format!(
                "user-auth request-code: {s}: {b}"
            )))
        }
    }

    pub async fn verify_code(
        &self,
        document: &str,
        code: &str,
    ) -> Result<VerifyResponse, AppError> {
        let resp = self
            .http
            .post(format!("{}/auth/verify-code", self.base_url))
            .json(&VerifyBody { document, code })
            .send()
            .await
            .map_err(|e| AppError::Downstream(format!("user-auth verify-code: {e}")))?;
        let status = resp.status();
        let bytes = resp.bytes().await.unwrap_or_default();
        if !status.is_success() {
            let body = String::from_utf8_lossy(&bytes);
            return Err(AppError::Downstream(format!(
                "user-auth verify-code: {status}: {body}"
            )));
        }
        serde_json::from_slice::<VerifyResponse>(&bytes)
            .map_err(|e| AppError::Downstream(format!("user-auth verify-code parse: {e}")))
    }
}
