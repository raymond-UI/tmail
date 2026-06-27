//! mail.tm receive backend (DESIGN.md §6).
//!
//! Bakes in the hard-won details: dual response shapes (array vs Hydra
//! collection), token refresh on 401, 429 → `RATE_LIMITED` with `Retry-After`,
//! and body hydration (the `html` array is joined; `text` is derived when the
//! provider omits it).

use async_trait::async_trait;
use reqwest::{Method, Response, StatusCode};
use serde::Deserialize;

use crate::error::{AppError, ErrorCode, Result};
use crate::http;
use crate::model::{Handle, InboxRecord, Message};
use crate::receive::Receiver;
use crate::util;

const DEFAULT_BASE: &str = "https://api.mail.tm";

/// The mail.tm transport.
pub struct MailTm {
    client: reqwest::Client,
    base: String,
}

impl MailTm {
    /// Construct against the public mail.tm API.
    pub fn new(client: reqwest::Client) -> Self {
        MailTm {
            client,
            base: DEFAULT_BASE.to_string(),
        }
    }

    /// Construct against a custom base URL (used by tests / mocks).
    pub fn with_base(client: reqwest::Client, base: impl Into<String>) -> Self {
        MailTm {
            client,
            base: base.into(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    /// Pick an active, non-private domain (DESIGN.md §6).
    async fn pick_domain(&self) -> Result<String> {
        let resp = self.client.get(self.url("/domains")).send().await?;
        let resp = ensure_ok(resp).await?;
        let coll: Collection<ApiDomain> = resp.json().await?;
        coll.into_vec()
            .into_iter()
            .find(|d| d.is_active && !d.is_private)
            .map(|d| d.domain)
            .ok_or_else(|| {
                AppError::new(
                    ErrorCode::AllProvidersDown,
                    "no active public mail.tm domain available",
                )
            })
    }

    /// Create an account, returning its server-side id and canonical address.
    async fn create_account(&self, address: &str, password: &str) -> Result<ApiAccount> {
        let resp = self
            .client
            .post(self.url("/accounts"))
            .json(&serde_json::json!({ "address": address, "password": password }))
            .send()
            .await?;
        let resp = ensure_ok(resp).await?;
        Ok(resp.json().await?)
    }

    /// Exchange credentials for a bearer token.
    async fn token(&self, address: &str, password: &str) -> Result<String> {
        let resp = self
            .client
            .post(self.url("/token"))
            .json(&serde_json::json!({ "address": address, "password": password }))
            .send()
            .await?;
        let resp = ensure_ok(resp).await?;
        let t: ApiToken = resp.json().await?;
        Ok(t.token)
    }

    /// Send an authenticated request, refreshing the token once on a 401
    /// (DESIGN.md §6). Returns the raw response for the caller to interpret.
    async fn authed(&self, method: Method, path: &str, handle: &Handle) -> Result<Response> {
        let url = self.url(path);
        let resp = self
            .client
            .request(method.clone(), &url)
            .bearer_auth(&handle.token)
            .send()
            .await?;
        if resp.status() == StatusCode::UNAUTHORIZED {
            let fresh = self.token(&handle.address, &handle.password).await?;
            let resp = self
                .client
                .request(method, &url)
                .bearer_auth(fresh)
                .send()
                .await?;
            return Ok(resp);
        }
        Ok(resp)
    }
}

#[async_trait]
impl Receiver for MailTm {
    async fn new_inbox(&self) -> Result<InboxRecord> {
        let domain = self.pick_domain().await?;
        let address = format!("{}@{}", util::gen_local_part(), domain);
        let password = util::gen_password();

        let account = self.create_account(&address, &password).await?;
        let token = self.token(&account.address, &password).await?;

        let handle = Handle {
            account_id: account.id,
            address: account.address.clone(),
            password,
            token,
        };
        Ok(InboxRecord {
            id: util::gen_short_id(),
            address: account.address,
            provider: "mail.tm".to_string(),
            handle,
            created_at: util::now_rfc3339(),
        })
    }

    async fn read(&self, handle: &Handle) -> Result<Vec<Message>> {
        let resp = self.authed(Method::GET, "/messages", handle).await?;
        let resp = ensure_ok(resp).await?;
        let coll: Collection<ApiMsgSummary> = resp.json().await?;
        Ok(coll.into_vec().into_iter().map(Message::from).collect())
    }

    async fn get(&self, handle: &Handle, msg_id: &str) -> Result<Message> {
        let path = format!("/messages/{msg_id}");
        let resp = self.authed(Method::GET, &path, handle).await?;
        let resp = ensure_ok(resp).await?;
        let full: ApiMsgFull = resp.json().await?;
        Ok(Message::from(full))
    }

    async fn delete(&self, handle: &Handle) -> Result<()> {
        let path = format!("/accounts/{}", handle.account_id);
        let resp = self.authed(Method::DELETE, &path, handle).await?;
        // Already-gone is success for an idempotent delete.
        if resp.status() == StatusCode::NOT_FOUND {
            return Ok(());
        }
        ensure_ok(resp).await?;
        Ok(())
    }
}

/// Map a non-success status to a typed [`AppError`]; pass success through.
async fn ensure_ok(resp: Response) -> Result<Response> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let retry = http::parse_retry_after(resp.headers());
    let body = resp.text().await.unwrap_or_default();
    let snippet = body.chars().take(200).collect::<String>();
    Err(match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            AppError::auth(format!("mail.tm rejected credentials ({status})"))
        }
        StatusCode::NOT_FOUND => AppError::not_found(format!("mail.tm: not found ({status})")),
        StatusCode::TOO_MANY_REQUESTS => {
            AppError::rate_limited("mail.tm is rate limiting account requests", retry)
        }
        _ => AppError::new(ErrorCode::Generic, format!("mail.tm {status}: {snippet}")),
    })
}

/// Render an HTML body to readable plain text for the `text` field / otp scan.
fn render_text(html: &str) -> String {
    html2text::from_read(html.as_bytes(), 80)
}

// ---- mail.tm wire types --------------------------------------------------

/// A list response that may be a plain JSON array *or* a Hydra collection
/// (`{ "hydra:member": [...] }`) depending on the `Accept` header (DESIGN.md §6).
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Collection<T> {
    Hydra(HydraCollection<T>),
    Array(Vec<T>),
}

impl<T> Collection<T> {
    fn into_vec(self) -> Vec<T> {
        match self {
            Collection::Hydra(h) => h.member,
            Collection::Array(v) => v,
        }
    }
}

#[derive(Debug, Deserialize)]
struct HydraCollection<T> {
    #[serde(rename = "hydra:member")]
    member: Vec<T>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiDomain {
    domain: String,
    is_active: bool,
    is_private: bool,
}

#[derive(Debug, Deserialize)]
struct ApiAccount {
    id: String,
    address: String,
}

#[derive(Debug, Deserialize)]
struct ApiToken {
    token: String,
}

#[derive(Debug, Deserialize)]
struct ApiAddress {
    #[serde(default)]
    address: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiMsgSummary {
    id: String,
    from: ApiAddress,
    #[serde(default)]
    subject: Option<String>,
    #[serde(default)]
    intro: Option<String>,
    #[serde(default)]
    seen: bool,
    created_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiMsgFull {
    id: String,
    from: ApiAddress,
    #[serde(default)]
    subject: Option<String>,
    #[serde(default)]
    intro: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    html: Option<Vec<String>>,
    #[serde(default)]
    seen: bool,
    created_at: String,
}

impl From<ApiMsgSummary> for Message {
    fn from(m: ApiMsgSummary) -> Self {
        Message {
            id: m.id,
            from: m.from.address,
            subject: m.subject.unwrap_or_default(),
            intro: m.intro.unwrap_or_default(),
            text: String::new(),
            html: None,
            date: m.created_at,
            seen: m.seen,
        }
    }
}

impl From<ApiMsgFull> for Message {
    fn from(m: ApiMsgFull) -> Self {
        let html = m
            .html
            .filter(|v| !v.is_empty())
            .map(|v| v.join("\n"))
            .filter(|s| !s.trim().is_empty());
        let text = match m.text {
            Some(t) if !t.trim().is_empty() => t,
            _ => html.as_deref().map(render_text).unwrap_or_default(),
        };
        Message {
            id: m.id,
            from: m.from.address,
            subject: m.subject.unwrap_or_default(),
            intro: m.intro.unwrap_or_default(),
            text,
            html,
            date: m.created_at,
            seen: m.seen,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_array_collection() {
        let json = r#"[{"domain":"a.com","isActive":true,"isPrivate":false}]"#;
        let c: Collection<ApiDomain> = serde_json::from_str(json).unwrap();
        let v = c.into_vec();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].domain, "a.com");
    }

    #[test]
    fn parses_hydra_collection() {
        let json = r#"{"hydra:member":[{"domain":"b.com","isActive":true,"isPrivate":false}],"hydra:totalItems":1}"#;
        let c: Collection<ApiDomain> = serde_json::from_str(json).unwrap();
        let v = c.into_vec();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].domain, "b.com");
    }

    #[test]
    fn summary_maps_from_address_and_seen() {
        let json = r#"{"id":"m1","from":{"address":"x@y.com"},"subject":"Hi","intro":"yo","seen":false,"createdAt":"2026-06-27T18:41:12Z"}"#;
        let s: ApiMsgSummary = serde_json::from_str(json).unwrap();
        let m = Message::from(s);
        assert_eq!(m.from, "x@y.com");
        assert_eq!(m.subject, "Hi");
        assert!(!m.seen);
        assert!(m.text.is_empty());
    }

    #[test]
    fn full_joins_html_array_and_derives_text() {
        let json = r#"{"id":"m1","from":{"address":"x@y.com"},"subject":"S","text":"","html":["<p>Code 1234</p>","<p>bye</p>"],"seen":true,"createdAt":"2026-06-27T18:41:12Z"}"#;
        let f: ApiMsgFull = serde_json::from_str(json).unwrap();
        let m = Message::from(f);
        assert!(m.html.as_ref().unwrap().contains("Code 1234"));
        // text derived from html when the provider sends an empty text part
        assert!(m.text.contains("1234"));
    }

    #[test]
    fn full_prefers_provided_text() {
        let json = r#"{"id":"m1","from":{"address":"x@y.com"},"text":"plain body","html":["<p>html body</p>"],"seen":false,"createdAt":"2026-06-27T18:41:12Z"}"#;
        let f: ApiMsgFull = serde_json::from_str(json).unwrap();
        let m = Message::from(f);
        assert_eq!(m.text, "plain body");
    }
}
