//! Gmail REST client and OAuth 2.0 (installed app, loopback redirect, PKCE).

pub mod auth;
pub mod model;

use model::*;
use reqwest::StatusCode;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("gmail api {status}: {body}")]
    Api { status: u16, body: String },
    /// `startHistoryId` is too old; do a full resync (Gmail returns 404).
    #[error("history id expired, full resync needed")]
    HistoryExpired,
    #[error("auth: {0}")]
    Auth(String),
}
pub type Result<T> = std::result::Result<T, Error>;

pub const DEFAULT_BASE: &str = "https://gmail.googleapis.com/gmail/v1/users/me";

pub struct Client {
    http: reqwest::Client,
    base: String,
    token: String,
}

impl Client {
    pub fn new(access_token: impl Into<String>) -> Self {
        Self::with_base(DEFAULT_BASE, access_token)
    }

    pub fn with_base(base: impl Into<String>, access_token: impl Into<String>) -> Self {
        Self { http: reqwest::Client::new(), base: base.into(), token: access_token.into() }
    }

    pub fn set_token(&mut self, t: impl Into<String>) {
        self.token = t.into();
    }

    async fn send<T: serde::de::DeserializeOwned>(&self, rb: reqwest::RequestBuilder) -> Result<T> {
        let r = rb.bearer_auth(&self.token).send().await?;
        let status = r.status();
        if status.is_success() {
            return Ok(r.json().await?);
        }
        let body = r.text().await.unwrap_or_default();
        Err(Error::Api { status: status.as_u16(), body })
    }

    /// Newest-first page of message ids for a label.
    pub async fn list_messages(&self, label: &str, page_token: Option<&str>, max: u32) -> Result<MessageList> {
        let mut q = vec![("labelIds", label.to_string()), ("maxResults", max.to_string())];
        if let Some(t) = page_token {
            q.push(("pageToken", t.to_string()));
        }
        self.send(self.http.get(format!("{}/messages", self.base)).query(&q)).await
    }

    /// Headers-only fetch: enough for the thread list, small on the wire.
    pub async fn get_metadata(&self, id: &str) -> Result<Message> {
        let q = [
            ("format", "metadata"),
            ("metadataHeaders", "From"),
            ("metadataHeaders", "Subject"),
            ("metadataHeaders", "Date"),
        ];
        self.send(self.http.get(format!("{}/messages/{id}", self.base)).query(&q)).await
    }

    /// Full RFC 822 source, base64url in `raw`.
    pub async fn get_raw(&self, id: &str) -> Result<Message> {
        self.send(self.http.get(format!("{}/messages/{id}", self.base)).query(&[("format", "raw")])).await
    }

    /// Incremental sync. Maps Gmail's 404 to `HistoryExpired`.
    pub async fn history(&self, start: u64, page_token: Option<&str>) -> Result<HistoryList> {
        let mut q = vec![("startHistoryId", start.to_string())];
        if let Some(t) = page_token {
            q.push(("pageToken", t.to_string()));
        }
        match self.send(self.http.get(format!("{}/history", self.base)).query(&q)).await {
            Err(Error::Api { status, .. }) if status == StatusCode::NOT_FOUND.as_u16() => Err(Error::HistoryExpired),
            other => other,
        }
    }

    pub async fn modify(&self, id: &str, add: &[&str], remove: &[&str]) -> Result<Message> {
        let body = serde_json::json!({ "addLabelIds": add, "removeLabelIds": remove });
        self.send(self.http.post(format!("{}/messages/{id}/modify", self.base)).json(&body)).await
    }

    pub async fn profile(&self) -> Result<Profile> {
        self.send(self.http.get(format!("{}/profile", self.base))).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn lists_and_fetches_metadata() {
        let s = MockServer::start().await;
        Mock::given(method("GET")).and(path("/messages")).and(query_param("labelIds", "INBOX")).and(header("authorization", "Bearer tok"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "messages": [{"id": "a", "threadId": "t1"}], "nextPageToken": "n2"
            }))).mount(&s).await;
        Mock::given(method("GET")).and(path("/messages/a")).and(query_param("format", "metadata"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "a", "threadId": "t1", "historyId": "77", "internalDate": "1700000000000",
                "labelIds": ["INBOX", "UNREAD"], "snippet": "hi",
                "payload": {"headers": [
                    {"name": "From", "value": "Ada Lovelace <ada@example.com>"},
                    {"name": "Subject", "value": "Hello"}]}
            }))).mount(&s).await;
        let c = Client::with_base(s.uri(), "tok");
        let l = c.list_messages("INBOX", None, 50).await.unwrap();
        assert_eq!(l.messages[0].id, "a");
        assert_eq!(l.next_page_token.as_deref(), Some("n2"));
        let m = c.get_metadata("a").await.unwrap();
        assert_eq!(m.history_id, 77);
        assert_eq!(m.internal_date, 1_700_000_000_000);
        assert!(m.is_unread());
        assert_eq!(m.header("subject"), Some("Hello"));
        assert_eq!(m.from(), ("Ada Lovelace".to_string(), "ada@example.com".to_string()));
    }

    #[tokio::test]
    async fn history_404_means_resync() {
        let s = MockServer::start().await;
        Mock::given(method("GET")).and(path("/history")).respond_with(ResponseTemplate::new(404)).mount(&s).await;
        let c = Client::with_base(s.uri(), "tok");
        assert!(matches!(c.history(1, None).await, Err(Error::HistoryExpired)));
    }

    #[tokio::test]
    async fn history_parses_changes() {
        let s = MockServer::start().await;
        Mock::given(method("GET")).and(path("/history")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "historyId": "90",
            "history": [{"id": "88",
                "messagesAdded": [{"message": {"id": "x", "threadId": "t"}}],
                "messagesDeleted": [{"message": {"id": "y", "threadId": "t"}}],
                "labelsAdded": [{"message": {"id": "z", "threadId": "t"}, "labelIds": ["STARRED"]}]}]
        }))).mount(&s).await;
        let h = Client::with_base(s.uri(), "tok").history(80, None).await.unwrap();
        assert_eq!(h.history_id, 90);
        assert_eq!(h.history[0].messages_added[0].message.id, "x");
        assert_eq!(h.history[0].messages_deleted[0].message.id, "y");
        assert_eq!(h.history[0].labels_added[0].label_ids, vec!["STARRED"]);
    }

    #[tokio::test]
    async fn modify_sends_label_changes() {
        let s = MockServer::start().await;
        Mock::given(method("POST")).and(path("/messages/a/modify"))
            .and(wiremock::matchers::body_json(serde_json::json!({"addLabelIds": [], "removeLabelIds": ["INBOX"]})))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"id": "a", "threadId": "t"}))).mount(&s).await;
        Client::with_base(s.uri(), "tok").modify("a", &[], &["INBOX"]).await.unwrap();
    }
}
