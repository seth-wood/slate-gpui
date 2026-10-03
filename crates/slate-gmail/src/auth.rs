//! OAuth 2.0 for installed apps: loopback redirect + PKCE (RFC 7636/8252).
//! Scope is `gmail.modify`: read, label, draft, send; no permanent delete.

use crate::{Error, Result};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use rand::RngCore;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

pub const SCOPE: &str = "https://www.googleapis.com/auth/gmail.modify";
pub const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
pub const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

#[derive(Debug, Clone)]
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

fn random_b64(n: usize) -> String {
    let mut b = vec![0u8; n];
    rand::rng().fill_bytes(&mut b);
    B64.encode(b)
}

impl Pkce {
    pub fn new() -> Self {
        let verifier = random_b64(48);
        let challenge = Self::challenge_for(&verifier);
        Self { verifier, challenge }
    }
    pub fn challenge_for(verifier: &str) -> String {
        B64.encode(Sha256::digest(verifier.as_bytes()))
    }
}

impl Default for Pkce {
    fn default() -> Self { Self::new() }
}

pub fn authorize_url(client_id: &str, redirect: &str, pkce: &Pkce, state: &str) -> String {
    let mut u = url::Url::parse(AUTH_URL).unwrap();
    u.query_pairs_mut()
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", redirect)
        .append_pair("response_type", "code")
        .append_pair("scope", SCOPE)
        .append_pair("code_challenge", &pkce.challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("access_type", "offline")
        .append_pair("prompt", "consent")
        .append_pair("state", state);
    u.into()
}

#[derive(Debug, Clone, Deserialize)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: u64,
}

/// Bind a loopback port, wait for Google's redirect, return the `code`.
/// Verifies `state`. Returns after answering the browser with a small page.
pub struct Loopback {
    listener: TcpListener,
}

impl Loopback {
    pub async fn bind() -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await.map_err(|e| Error::Auth(e.to_string()))?;
        Ok(Self { listener })
    }
    pub fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}", self.listener.local_addr().unwrap().port())
    }
    pub async fn wait_for_code(self, expected_state: &str) -> Result<String> {
        let (mut s, _) = self.listener.accept().await.map_err(|e| Error::Auth(e.to_string()))?;
        let mut buf = vec![0u8; 8192];
        let n = s.read(&mut buf).await.map_err(|e| Error::Auth(e.to_string()))?;
        let req = String::from_utf8_lossy(&buf[..n]);
        let target = req.lines().next().and_then(|l| l.split(' ').nth(1)).unwrap_or("/");
        let u = url::Url::parse(&format!("http://localhost{target}")).map_err(|e| Error::Auth(e.to_string()))?;
        let q: std::collections::HashMap<_, _> = u.query_pairs().into_owned().collect();
        let ok = q.get("state").map(String::as_str) == Some(expected_state) && q.contains_key("code");
        let body = if ok { "Signed in to Slate. You can close this tab." } else { "Sign-in failed. Return to Slate." };
        let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        let _ = s.write_all(resp.as_bytes()).await;
        if !ok {
            return Err(Error::Auth(q.get("error").cloned().unwrap_or_else(|| "state mismatch".into())));
        }
        Ok(q["code"].clone())
    }
}

pub async fn exchange_code(token_url: &str, client_id: &str, client_secret: &str, code: &str, redirect: &str, verifier: &str) -> Result<Tokens> {
    token_request(token_url, &[
        ("grant_type", "authorization_code"), ("code", code), ("redirect_uri", redirect),
        ("client_id", client_id), ("client_secret", client_secret), ("code_verifier", verifier),
    ]).await
}

pub async fn refresh(token_url: &str, client_id: &str, client_secret: &str, refresh_token: &str) -> Result<Tokens> {
    token_request(token_url, &[
        ("grant_type", "refresh_token"), ("refresh_token", refresh_token),
        ("client_id", client_id), ("client_secret", client_secret),
    ]).await
}

async fn token_request(url: &str, form: &[(&str, &str)]) -> Result<Tokens> {
    let r = reqwest::Client::new().post(url).form(form).send().await?;
    if !r.status().is_success() {
        return Err(Error::Auth(format!("{}: {}", r.status(), r.text().await.unwrap_or_default())));
    }
    Ok(r.json().await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc7636_challenge_vector() {
        // Appendix B of RFC 7636.
        assert_eq!(
            Pkce::challenge_for("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn authorize_url_has_pkce_and_offline() {
        let p = Pkce::new();
        let u = authorize_url("cid", "http://127.0.0.1:5000", &p, "st");
        assert!(u.contains("code_challenge_method=S256") && u.contains("access_type=offline"));
        assert!(u.contains(&format!("code_challenge={}", p.challenge)));
        assert!(u.contains("gmail.modify"));
    }

    #[tokio::test]
    async fn loopback_returns_code_and_rejects_bad_state() {
        let lb = Loopback::bind().await.unwrap();
        let port = lb.redirect_uri();
        let t = tokio::spawn(lb.wait_for_code("good"));
        reqwest::get(format!("{port}/?code=abc&state=good")).await.unwrap();
        assert_eq!(t.await.unwrap().unwrap(), "abc");

        let lb = Loopback::bind().await.unwrap();
        let port = lb.redirect_uri();
        let t = tokio::spawn(lb.wait_for_code("good"));
        reqwest::get(format!("{port}/?code=abc&state=evil")).await.unwrap();
        assert!(t.await.unwrap().is_err());
    }

    #[tokio::test]
    async fn exchanges_code_for_tokens() {
        use wiremock::matchers::{body_string_contains, method};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let s = MockServer::start().await;
        Mock::given(method("POST")).and(body_string_contains("code_verifier=v"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "at", "refresh_token": "rt", "expires_in": 3599
            }))).mount(&s).await;
        let t = exchange_code(&s.uri(), "cid", "sec", "code", "http://127.0.0.1:1", "v").await.unwrap();
        assert_eq!((t.access_token.as_str(), t.refresh_token.as_deref()), ("at", Some("rt")));
    }
}
