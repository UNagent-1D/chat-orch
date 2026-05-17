//! Secure-channel primitives shared by every outbound HTTP client in this
//! service. AES-256-GCM via OpenSSL, with the JSON envelope described in
//! the umbrella `CLAUDE.md` (Secure Channel pattern).
//!
//! Outbound requests run through [`Channel::apply_request`], which seals the
//! body and sets `Content-Type: application/vnd.unagent.secure+json` +
//! `X-Secure-Channel: aes256gcm/1` when encryption is active. Responses are
//! parsed via [`Channel::decode_response`] which transparently decrypts when
//! the callee replies in envelope form and falls back to plaintext otherwise.

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use openssl::rand::rand_bytes;
use openssl::symm::{decrypt_aead, encrypt_aead, Cipher};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::error::AppError;

pub const HEADER_NAME: &str = "X-Secure-Channel";
pub const HEADER_VALUE: &str = "aes256gcm/1";
pub const CONTENT_TYPE: &str = "application/vnd.unagent.secure+json";
const IV_LEN: usize = 12;
const TAG_LEN: usize = 16;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub v: u8,
    pub iv: String,
    pub ct: String,
    pub tag: String,
}

#[derive(Debug, Clone)]
pub struct ChannelKey {
    bytes: [u8; 32],
}

impl ChannelKey {
    pub fn from_base64(s: &str) -> Result<Self, AppError> {
        let decoded = B64.decode(s.trim()).map_err(|e| {
            AppError::Internal(format!("BACKEND_CHANNEL_KEY invalid base64: {e}"))
        })?;
        if decoded.len() != 32 {
            return Err(AppError::Internal(format!(
                "BACKEND_CHANNEL_KEY must decode to 32 bytes, got {}",
                decoded.len()
            )));
        }
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&decoded);
        Ok(Self { bytes })
    }

    pub fn seal(&self, plaintext: &[u8]) -> Result<Envelope, AppError> {
        let mut iv = [0u8; IV_LEN];
        rand_bytes(&mut iv)
            .map_err(|e| AppError::Internal(format!("rand_bytes: {e}")))?;
        let mut tag = [0u8; TAG_LEN];
        let ct = encrypt_aead(
            Cipher::aes_256_gcm(),
            &self.bytes,
            Some(&iv),
            &[],
            plaintext,
            &mut tag,
        )
        .map_err(|e| AppError::Internal(format!("aes-256-gcm encrypt: {e}")))?;
        Ok(Envelope {
            v: 1,
            iv: B64.encode(iv),
            ct: B64.encode(ct),
            tag: B64.encode(tag),
        })
    }

    pub fn open(&self, env: &Envelope) -> Result<Vec<u8>, AppError> {
        if env.v != 1 {
            return Err(AppError::BadRequest(format!(
                "unsupported channel envelope version {}",
                env.v
            )));
        }
        let iv = B64
            .decode(env.iv.as_bytes())
            .map_err(|e| AppError::BadRequest(format!("envelope iv invalid base64: {e}")))?;
        let ct = B64
            .decode(env.ct.as_bytes())
            .map_err(|e| AppError::BadRequest(format!("envelope ct invalid base64: {e}")))?;
        let tag = B64
            .decode(env.tag.as_bytes())
            .map_err(|e| AppError::BadRequest(format!("envelope tag invalid base64: {e}")))?;
        if iv.len() != IV_LEN {
            return Err(AppError::BadRequest("iv must decode to 12 bytes".into()));
        }
        if tag.len() != TAG_LEN {
            return Err(AppError::BadRequest("tag must decode to 16 bytes".into()));
        }
        decrypt_aead(
            Cipher::aes_256_gcm(),
            &self.bytes,
            Some(&iv),
            &[],
            &ct,
            &tag,
        )
        .map_err(|_| AppError::BadRequest("AEAD verification failed".into()))
    }
}

/// A `Channel` is the runtime handle used by every outbound HTTP client.
/// It encapsulates both the optional key material and the kill switch so
/// callers can stay agnostic of whether encryption is currently enabled.
#[derive(Debug, Clone)]
pub struct Channel {
    key: Option<ChannelKey>,
    enabled: bool,
}

impl Channel {
    pub fn new(key: Option<ChannelKey>, enabled: bool) -> Result<Self, AppError> {
        if enabled && key.is_none() {
            return Err(AppError::MissingEnv("BACKEND_CHANNEL_KEY".into()));
        }
        Ok(Self { key, enabled })
    }

    /// Disabled channel — useful in tests and during bootstrap before the
    /// flag has been flipped on across the stack.
    pub fn disabled() -> Self {
        Self {
            key: None,
            enabled: false,
        }
    }

    pub fn active(&self) -> bool {
        self.enabled && self.key.is_some()
    }

    /// Serialise `body` to JSON and, when active, seal it. Returns
    /// (bytes-on-wire, content-type, whether-encrypted).
    pub fn seal_json<B: Serialize>(
        &self,
        body: &B,
    ) -> Result<(Vec<u8>, &'static str, bool), AppError> {
        let plain = serde_json::to_vec(body)
            .map_err(|e| AppError::Internal(format!("serialize body: {e}")))?;
        if !self.active() {
            return Ok((plain, "application/json", false));
        }
        let key = self.key.as_ref().expect("active() guarantees key");
        let env = key.seal(&plain)?;
        let bytes = serde_json::to_vec(&env)
            .map_err(|e| AppError::Internal(format!("serialize envelope: {e}")))?;
        Ok((bytes, CONTENT_TYPE, true))
    }

    /// Attach a serialised + (optionally) sealed body and the right headers
    /// to a `reqwest::RequestBuilder`.
    pub fn apply_request<B: Serialize>(
        &self,
        req: reqwest::RequestBuilder,
        body: &B,
    ) -> Result<reqwest::RequestBuilder, AppError> {
        let (bytes, content_type, encrypted) = self.seal_json(body)?;
        let mut req = req
            .header(reqwest::header::CONTENT_TYPE, content_type)
            .body(bytes);
        if encrypted {
            req = req.header(HEADER_NAME, HEADER_VALUE);
        }
        Ok(req)
    }

    /// Read a `reqwest::Response`, decrypt the body if the callee replied in
    /// envelope form, and decode the JSON. Non-2xx is surfaced as a
    /// `Downstream` error with a short body preview.
    pub async fn decode_response<T: DeserializeOwned>(
        &self,
        resp: reqwest::Response,
    ) -> Result<T, AppError> {
        let status = resp.status();
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(String::from);
        let bytes = resp.bytes().await?;
        if !status.is_success() {
            // Best-effort decryption of error body for clearer logs.
            let plain = self
                .open_bytes(content_type.as_deref(), &bytes)
                .unwrap_or_else(|_| bytes.to_vec());
            let preview: String = String::from_utf8_lossy(&plain)
                .chars()
                .take(200)
                .collect();
            return Err(AppError::Downstream(format!("status {status}: {preview}")));
        }
        let plain = self.open_bytes(content_type.as_deref(), &bytes)?;
        serde_json::from_slice(&plain)
            .map_err(|e| AppError::Downstream(format!("decode response json: {e}")))
    }

    /// Decrypt raw response bytes when the content-type marks them as an
    /// envelope; otherwise return the bytes unchanged. Useful when the caller
    /// needs the response bytes before JSON-decoding (e.g. an HTTP client that
    /// preserves non-2xx bodies as `Value` for the LLM to inspect).
    pub fn open_bytes(
        &self,
        content_type: Option<&str>,
        bytes: &[u8],
    ) -> Result<Vec<u8>, AppError> {
        let looks_secure = content_type
            .map(|c| c.starts_with(CONTENT_TYPE))
            .unwrap_or(false);
        if !looks_secure {
            return Ok(bytes.to_vec());
        }
        let key = self.key.as_ref().ok_or_else(|| {
            AppError::Internal(
                "received secure envelope but BACKEND_CHANNEL_KEY is not configured".into(),
            )
        })?;
        let env: Envelope = serde_json::from_slice(bytes)
            .map_err(|e| AppError::Downstream(format!("envelope parse: {e}")))?;
        key.open(&env)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Fixed vector copied from scripts/channel-vector.json. Asserting against
    // it proves Rust agrees on the wire format with the other languages.
    const KEY_B64: &str = "ASNFZ4mrze8BI0VniavN7wEjRWeJq83vASNFZ4mrze8=";
    const IV_B64: &str = "CgsMDQ4PEBESExQV";
    const PLAINTEXT_B64: &str = "eyJoZWxsbyI6IndvcmxkIiwidGVuYW50IjoiZGVtbyIsIm4iOjQyfQ==";
    const CIPHERTEXT_B64: &str = "HA/jiyD9Ct203QHgmTq3vFe0VYYm5ynGin9Xn7B3QVGAZkDXaJLrEQ==";
    const TAG_B64: &str = "XjADTXgk4M//4jqs0Cu05g==";

    #[test]
    fn vector_decrypts_to_known_plaintext() {
        let key = ChannelKey::from_base64(KEY_B64).unwrap();
        let env = Envelope {
            v: 1,
            iv: IV_B64.into(),
            ct: CIPHERTEXT_B64.into(),
            tag: TAG_B64.into(),
        };
        let plain = key.open(&env).expect("open should succeed");
        assert_eq!(plain, B64.decode(PLAINTEXT_B64).unwrap());
    }

    #[test]
    fn vector_seal_with_fixed_iv_matches() {
        // Direct call into openssl to lock the IV (Channel::seal generates one).
        let key_bytes = B64.decode(KEY_B64).unwrap();
        let iv = B64.decode(IV_B64).unwrap();
        let pt = B64.decode(PLAINTEXT_B64).unwrap();
        let mut tag = [0u8; TAG_LEN];
        let ct = encrypt_aead(
            Cipher::aes_256_gcm(),
            &key_bytes,
            Some(&iv),
            &[],
            &pt,
            &mut tag,
        )
        .unwrap();
        assert_eq!(B64.encode(ct), CIPHERTEXT_B64);
        assert_eq!(B64.encode(tag), TAG_B64);
    }

    #[test]
    fn roundtrip_preserves_payload() {
        let channel = Channel::new(Some(ChannelKey::from_base64(KEY_B64).unwrap()), true).unwrap();
        let body = serde_json::json!({ "session": "abc", "n": 7, "msg": "hola" });
        let (wire, ct, encrypted) = channel.seal_json(&body).unwrap();
        assert!(encrypted);
        assert_eq!(ct, CONTENT_TYPE);

        let env: Envelope = serde_json::from_slice(&wire).unwrap();
        let plain = channel.key.as_ref().unwrap().open(&env).unwrap();
        let back: serde_json::Value = serde_json::from_slice(&plain).unwrap();
        assert_eq!(back, body);
    }

    #[test]
    fn tampered_ciphertext_fails_aead() {
        let key = ChannelKey::from_base64(KEY_B64).unwrap();
        let env = Envelope {
            v: 1,
            iv: IV_B64.into(),
            // flip the last base64 char to break the ciphertext
            ct: {
                let mut s = CIPHERTEXT_B64.to_string();
                let last = s.pop().unwrap();
                s.push(if last == 'A' { 'B' } else { 'A' });
                s
            },
            tag: TAG_B64.into(),
        };
        let err = key.open(&env).expect_err("tampered ct must fail");
        match err {
            AppError::BadRequest(msg) => assert!(msg.contains("AEAD")),
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[test]
    fn disabled_channel_passes_plaintext() {
        let channel = Channel::disabled();
        let (bytes, ct, encrypted) =
            channel.seal_json(&serde_json::json!({ "a": 1 })).unwrap();
        assert!(!encrypted);
        assert_eq!(ct, "application/json");
        assert_eq!(bytes, b"{\"a\":1}");
    }
}
