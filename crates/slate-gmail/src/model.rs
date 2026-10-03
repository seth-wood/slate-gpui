use serde::{Deserialize, Deserializer};

fn num<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<u64, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum N { S(String), I(u64) }
    Ok(match N::deserialize(d)? {
        N::S(s) => s.parse().map_err(serde::de::Error::custom)?,
        N::I(i) => i,
    })
}
fn num_i<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<i64, D::Error> {
    num(d).map(|n| n as i64)
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MessageRef {
    pub id: String,
    #[serde(default)]
    pub thread_id: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MessageList {
    #[serde(default)]
    pub messages: Vec<MessageRef>,
    pub next_page_token: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Header {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Payload {
    #[serde(default)]
    pub headers: Vec<Header>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: String,
    #[serde(default)]
    pub thread_id: String,
    #[serde(default, deserialize_with = "num")]
    pub history_id: u64,
    #[serde(default, deserialize_with = "num_i")]
    pub internal_date: i64,
    #[serde(default)]
    pub label_ids: Vec<String>,
    #[serde(default)]
    pub snippet: String,
    pub payload: Option<Payload>,
    /// base64url RFC 822 source when fetched with `format=raw`.
    pub raw: Option<String>,
}

impl Message {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.payload.as_ref()?.headers.iter().find(|h| h.name.eq_ignore_ascii_case(name)).map(|h| h.value.as_str())
    }
    pub fn is_unread(&self) -> bool {
        self.label_ids.iter().any(|l| l == "UNREAD")
    }
    /// (display name, address) from a `Name <addr>` or bare-address From header.
    pub fn from(&self) -> (String, String) {
        let v = self.header("From").unwrap_or("").trim();
        match (v.rfind('<'), v.ends_with('>')) {
            (Some(i), true) => (v[..i].trim().trim_matches('"').to_string(), v[i + 1..v.len() - 1].to_string()),
            _ => (String::new(), v.to_string()),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Wrapped {
    pub message: MessageRef,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LabelChange {
    pub message: MessageRef,
    #[serde(default)]
    pub label_ids: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct HistoryRecord {
    #[serde(default)]
    pub messages_added: Vec<Wrapped>,
    #[serde(default)]
    pub messages_deleted: Vec<Wrapped>,
    #[serde(default)]
    pub labels_added: Vec<LabelChange>,
    #[serde(default)]
    pub labels_removed: Vec<LabelChange>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct HistoryList {
    #[serde(default)]
    pub history: Vec<HistoryRecord>,
    #[serde(default, deserialize_with = "num")]
    pub history_id: u64,
    pub next_page_token: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub email_address: String,
    #[serde(deserialize_with = "num")]
    pub history_id: u64,
}
