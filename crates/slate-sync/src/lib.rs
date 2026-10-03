//! Gmail -> local store sync: an initial metadata pull, then incremental
//! updates from `history.list`. A 404 from history means the stored
//! historyId expired, so we fall back to a fresh initial sync.

use slate_gmail::{Client, Error as GmailError, model::Message};
use slate_store::{NewMessage, Store};
use std::collections::BTreeSet;
use tokio::task::JoinSet;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Gmail(#[from] GmailError),
    #[error(transparent)]
    Store(#[from] slate_store::Error),
    #[error("task: {0}")]
    Join(#[from] tokio::task::JoinError),
}
pub type Result<T> = std::result::Result<T, Error>;

const FETCH_CONCURRENCY: usize = 16;
const STORE_CHUNK: usize = 200;
pub const STATE_HISTORY: &str = "history_id";
pub const STATE_EMAIL: &str = "email";

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Stats {
    pub upserted: usize,
    pub deleted: usize,
    pub full_resync: bool,
}

pub fn to_new_message(m: &Message) -> NewMessage {
    let (from_name, from_addr) = m.from();
    NewMessage {
        id: m.id.clone(),
        thread_id: m.thread_id.clone(),
        history_id: m.history_id,
        date_ms: m.internal_date,
        from_name,
        from_addr,
        subject: m.header("Subject").unwrap_or("").to_string(),
        snippet: m.snippet.clone(),
        unread: m.is_unread(),
        labels: m.label_ids.clone(),
    }
}

/// Fetch metadata for `ids` with bounded concurrency. Messages deleted
/// between listing and fetching (404) are skipped.
async fn fetch_metadata(client: &Client, ids: Vec<String>) -> Result<Vec<Message>> {
    let mut out = Vec::with_capacity(ids.len());
    let mut set = JoinSet::new();
    let mut it = ids.into_iter();
    loop {
        while set.len() < FETCH_CONCURRENCY {
            let Some(id) = it.next() else { break };
            let c = client.clone();
            set.spawn(async move { c.get_metadata(&id).await });
        }
        let Some(res) = set.join_next().await else { break };
        match res? {
            Ok(m) => out.push(m),
            Err(GmailError::Api { status: 404, .. }) => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(out)
}

fn save(store: &mut Store, msgs: &[Message]) -> Result<usize> {
    let rows: Vec<_> = msgs.iter().map(to_new_message).collect();
    for chunk in rows.chunks(STORE_CHUNK) {
        store.upsert_messages(chunk)?;
    }
    Ok(rows.len())
}

/// Pull the newest `limit` messages of `label`. The mailbox historyId is read
/// first so anything arriving during the pull is caught by the next
/// incremental sync.
pub async fn initial_sync(client: &Client, store: &mut Store, label: &str, limit: usize) -> Result<Stats> {
    let profile = client.profile().await?;
    let mut ids = Vec::new();
    let mut token: Option<String> = None;
    while ids.len() < limit {
        let page = client.list_messages(label, token.as_deref(), 100.min((limit - ids.len()) as u32)).await?;
        ids.extend(page.messages.into_iter().map(|m| m.id));
        token = page.next_page_token;
        if token.is_none() {
            break;
        }
    }
    let mut upserted = 0;
    for chunk in ids.chunks(STORE_CHUNK) {
        let msgs = fetch_metadata(client, chunk.to_vec()).await?;
        upserted += save(store, &msgs)?;
    }
    store.set_state(STATE_HISTORY, &profile.history_id.to_string())?;
    store.set_state(STATE_EMAIL, &profile.email_address)?;
    Ok(Stats { upserted, deleted: 0, full_resync: true })
}

/// Apply changes since the stored historyId. Falls back to `initial_sync`
/// when there is no stored id or Gmail has expired it.
pub async fn sync(client: &Client, store: &mut Store, label: &str, initial_limit: usize) -> Result<Stats> {
    let Some(start) = store.state(STATE_HISTORY)?.and_then(|s| s.parse::<u64>().ok()) else {
        return initial_sync(client, store, label, initial_limit).await;
    };
    let mut touched = BTreeSet::new();
    let mut deleted = BTreeSet::new();
    let mut newest = start;
    let mut token: Option<String> = None;
    loop {
        let page = match client.history(start, token.as_deref()).await {
            Ok(p) => p,
            Err(GmailError::HistoryExpired) => return initial_sync(client, store, label, initial_limit).await,
            Err(e) => return Err(e.into()),
        };
        for rec in &page.history {
            for w in &rec.messages_added {
                deleted.remove(&w.message.id);
                touched.insert(w.message.id.clone());
            }
            for c in rec.labels_added.iter().chain(&rec.labels_removed) {
                if !deleted.contains(&c.message.id) {
                    touched.insert(c.message.id.clone());
                }
            }
            for w in &rec.messages_deleted {
                touched.remove(&w.message.id);
                deleted.insert(w.message.id.clone());
            }
        }
        newest = newest.max(page.history_id);
        token = page.next_page_token;
        if token.is_none() {
            break;
        }
    }
    for id in &deleted {
        store.delete_message(id)?;
    }
    let msgs = fetch_metadata(client, touched.into_iter().collect()).await?;
    let upserted = save(store, &msgs)?;
    store.set_state(STATE_HISTORY, &newest.to_string())?;
    Ok(Stats { upserted, deleted: deleted.len(), full_resync: false })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn meta(id: &str, date: i64, labels: &[&str]) -> serde_json::Value {
        json!({"id": id, "threadId": format!("t{id}"), "historyId": "5", "internalDate": date.to_string(),
               "labelIds": labels, "snippet": "snip",
               "payload": {"headers": [{"name": "From", "value": "Ada <a@x.com>"}, {"name": "Subject", "value": format!("S {id}")}]}})
    }

    async fn mock_get(s: &MockServer, id: &str, date: i64, labels: &[&str]) {
        Mock::given(method("GET")).and(path(format!("/messages/{id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(meta(id, date, labels))).mount(s).await;
    }

    async fn mock_profile(s: &MockServer, hid: &str) {
        Mock::given(method("GET")).and(path("/profile"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"emailAddress": "me@x.com", "historyId": hid}))).mount(s).await;
    }

    async fn mock_list(s: &MockServer, ids: &[&str]) {
        let msgs: Vec<_> = ids.iter().map(|i| json!({"id": i, "threadId": format!("t{i}")})).collect();
        Mock::given(method("GET")).and(path("/messages")).and(query_param("labelIds", "INBOX"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"messages": msgs}))).mount(s).await;
    }

    #[tokio::test]
    async fn initial_sync_fills_store_and_state() {
        let s = MockServer::start().await;
        mock_profile(&s, "100").await;
        mock_list(&s, &["a", "b"]).await;
        mock_get(&s, "a", 2000, &["INBOX", "UNREAD"]).await;
        mock_get(&s, "b", 1000, &["INBOX"]).await;
        let mut store = Store::open_in_memory().unwrap();
        let st = sync(&Client::with_base(s.uri(), "t"), &mut store, "INBOX", 50).await.unwrap();
        assert_eq!(st, Stats { upserted: 2, deleted: 0, full_resync: true });
        let rows = store.page("INBOX", None, 10).unwrap();
        assert_eq!(rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert!(rows[0].unread && !rows[1].unread);
        assert_eq!(rows[0].from_name, "Ada");
        assert_eq!(store.state(STATE_HISTORY).unwrap().as_deref(), Some("100"));
        assert_eq!(store.state(STATE_EMAIL).unwrap().as_deref(), Some("me@x.com"));
    }

    #[tokio::test]
    async fn incremental_applies_add_delete_and_label_change() {
        let s = MockServer::start().await;
        let mut store = Store::open_in_memory().unwrap();
        store.upsert_messages(&[
            NewMessage { id: "old".into(), thread_id: "t".into(), history_id: 1, date_ms: 10, from_name: "".into(), from_addr: "".into(),
                         subject: "".into(), snippet: "".into(), unread: false, labels: vec!["INBOX".into()] },
            NewMessage { id: "gone".into(), thread_id: "t".into(), history_id: 1, date_ms: 11, from_name: "".into(), from_addr: "".into(),
                         subject: "".into(), snippet: "".into(), unread: false, labels: vec!["INBOX".into()] },
        ]).unwrap();
        store.set_state(STATE_HISTORY, "100").unwrap();
        Mock::given(method("GET")).and(path("/history")).respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "historyId": "120",
            "history": [{"messagesAdded": [{"message": {"id": "new", "threadId": "t"}}]},
                        {"labelsRemoved": [{"message": {"id": "old", "threadId": "t"}, "labelIds": ["INBOX"]}]},
                        {"messagesDeleted": [{"message": {"id": "gone", "threadId": "t"}}]}]
        }))).mount(&s).await;
        mock_get(&s, "new", 500, &["INBOX"]).await;
        mock_get(&s, "old", 10, &["ARCHIVED"]).await;
        let st = sync(&Client::with_base(s.uri(), "t"), &mut store, "INBOX", 50).await.unwrap();
        assert_eq!(st, Stats { upserted: 2, deleted: 1, full_resync: false });
        let ids: Vec<_> = store.page("INBOX", None, 10).unwrap().into_iter().map(|r| r.id).collect();
        assert_eq!(ids, ["new"]);
        assert_eq!(store.state(STATE_HISTORY).unwrap().as_deref(), Some("120"));
    }

    #[tokio::test]
    async fn expired_history_triggers_full_resync() {
        let s = MockServer::start().await;
        let mut store = Store::open_in_memory().unwrap();
        store.set_state(STATE_HISTORY, "1").unwrap();
        Mock::given(method("GET")).and(path("/history")).respond_with(ResponseTemplate::new(404)).mount(&s).await;
        mock_profile(&s, "900").await;
        mock_list(&s, &["a"]).await;
        mock_get(&s, "a", 1, &["INBOX"]).await;
        let st = sync(&Client::with_base(s.uri(), "t"), &mut store, "INBOX", 50).await.unwrap();
        assert!(st.full_resync);
        assert_eq!(store.state(STATE_HISTORY).unwrap().as_deref(), Some("900"));
    }

    #[tokio::test]
    async fn message_deleted_mid_fetch_is_skipped() {
        let s = MockServer::start().await;
        mock_profile(&s, "1").await;
        mock_list(&s, &["a", "ghost"]).await;
        mock_get(&s, "a", 1, &["INBOX"]).await;
        Mock::given(method("GET")).and(path("/messages/ghost")).respond_with(ResponseTemplate::new(404)).mount(&s).await;
        let mut store = Store::open_in_memory().unwrap();
        let st = sync(&Client::with_base(s.uri(), "t"), &mut store, "INBOX", 50).await.unwrap();
        assert_eq!(st.upserted, 1);
    }
}
