//! Local SQLite store: the single source of truth the UI reads from.

use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}
pub type Result<T> = std::result::Result<T, Error>;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS messages (
    id          TEXT PRIMARY KEY,
    thread_id   TEXT NOT NULL,
    history_id  INTEGER NOT NULL,
    date_ms     INTEGER NOT NULL,
    from_name   TEXT NOT NULL,
    from_addr   TEXT NOT NULL,
    subject     TEXT NOT NULL,
    snippet     TEXT NOT NULL,
    unread      INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS message_labels (
    message_id TEXT NOT NULL,
    label      TEXT NOT NULL,
    date_ms    INTEGER NOT NULL,
    PRIMARY KEY (label, date_ms, message_id)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS idx_labels_msg ON message_labels(message_id);
CREATE TABLE IF NOT EXISTS bodies (
    message_id TEXT PRIMARY KEY,
    zbody      BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS sync_state (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
"#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMessage {
    pub id: String,
    pub thread_id: String,
    pub history_id: u64,
    pub date_ms: i64,
    pub from_name: String,
    pub from_addr: String,
    pub subject: String,
    pub snippet: String,
    pub unread: bool,
    pub labels: Vec<String>,
}

/// One row of the thread list, everything the list view needs and nothing more.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub id: String,
    pub thread_id: String,
    pub date_ms: i64,
    pub from_name: String,
    pub subject: String,
    pub snippet: String,
    pub unread: bool,
}

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "mmap_size", 256i64 * 1024 * 1024)?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
    }

    /// Insert or replace a batch of messages in one transaction.
    pub fn upsert_messages(&mut self, msgs: &[NewMessage]) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut del = tx.prepare_cached("DELETE FROM message_labels WHERE message_id = ?1")?;
            let mut ins = tx.prepare_cached(
                "INSERT OR REPLACE INTO messages
                 (id, thread_id, history_id, date_ms, from_name, from_addr, subject, snippet, unread)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            )?;
            let mut lab = tx.prepare_cached(
                "INSERT OR REPLACE INTO message_labels (message_id, label, date_ms) VALUES (?1,?2,?3)",
            )?;
            for m in msgs {
                del.execute([&m.id])?;
                ins.execute(params![
                    m.id, m.thread_id, m.history_id as i64, m.date_ms, m.from_name,
                    m.from_addr, m.subject, m.snippet, m.unread
                ])?;
                for l in &m.labels {
                    lab.execute(params![m.id, l, m.date_ms])?;
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn delete_message(&mut self, id: &str) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM message_labels WHERE message_id = ?1", [id])?;
        tx.execute("DELETE FROM bodies WHERE message_id = ?1", [id])?;
        tx.execute("DELETE FROM messages WHERE id = ?1", [id])?;
        tx.commit()?;
        Ok(())
    }

    /// Page of a label's messages, newest first, served from the covering
    /// (label, date_ms) index. `before` is a keyset cursor: (date_ms, id).
    pub fn page(&self, label: &str, before: Option<(i64, &str)>, limit: usize) -> Result<Vec<Row>> {
        let (d, i) = before.unwrap_or((i64::MAX, "\u{10FFFF}"));
        let mut st = self.conn.prepare_cached(
            "SELECT m.id, m.thread_id, m.date_ms, m.from_name, m.subject, m.snippet, m.unread
             FROM message_labels l JOIN messages m ON m.id = l.message_id
             WHERE l.label = ?1 AND (l.date_ms < ?2 OR (l.date_ms = ?2 AND l.message_id < ?3))
             ORDER BY l.date_ms DESC, l.message_id DESC
             LIMIT ?4",
        )?;
        let rows = st
            .query_map(params![label, d, i, limit as i64], |r| {
                Ok(Row {
                    id: r.get(0)?,
                    thread_id: r.get(1)?,
                    date_ms: r.get(2)?,
                    from_name: r.get(3)?,
                    subject: r.get(4)?,
                    snippet: r.get(5)?,
                    unread: r.get(6)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn count(&self, label: &str) -> Result<usize> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM message_labels WHERE label = ?1",
            [label],
            |r| r.get::<_, i64>(0),
        )? as usize)
    }

    pub fn set_body(&mut self, message_id: &str, body: &[u8]) -> Result<()> {
        let z = zstd::encode_all(body, 3)?;
        self.conn.execute(
            "INSERT OR REPLACE INTO bodies (message_id, zbody) VALUES (?1, ?2)",
            params![message_id, z],
        )?;
        Ok(())
    }

    pub fn body(&self, message_id: &str) -> Result<Option<Vec<u8>>> {
        let z: Option<Vec<u8>> = self
            .conn
            .query_row("SELECT zbody FROM bodies WHERE message_id = ?1", [message_id], |r| r.get(0))
            .optional()?;
        Ok(match z {
            Some(z) => Some(zstd::decode_all(&z[..])?),
            None => None,
        })
    }

    pub fn set_state(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO sync_state (key, value) VALUES (?1, ?2)",
            [key, value],
        )?;
        Ok(())
    }

    pub fn state(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM sync_state WHERE key = ?1", [key], |r| r.get(0))
            .optional()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(n: i64) -> NewMessage {
        NewMessage {
            id: format!("m{n:08}"),
            thread_id: format!("t{n}"),
            history_id: n as u64,
            date_ms: 1_700_000_000_000 + n,
            from_name: "Ada".into(),
            from_addr: "ada@example.com".into(),
            subject: format!("Subject {n}"),
            snippet: "hello".into(),
            unread: n % 2 == 0,
            labels: vec!["INBOX".into()],
        }
    }

    #[test]
    fn pages_newest_first_with_cursor() {
        let mut s = Store::open_in_memory().unwrap();
        let all: Vec<_> = (0..25).map(msg).collect();
        s.upsert_messages(&all).unwrap();
        let p1 = s.page("INBOX", None, 10).unwrap();
        assert_eq!(p1.len(), 10);
        assert_eq!(p1[0].subject, "Subject 24");
        let last = p1.last().unwrap();
        let p2 = s.page("INBOX", Some((last.date_ms, &last.id)), 10).unwrap();
        assert_eq!(p2[0].subject, "Subject 14");
        let p3 = s.page("INBOX", Some((p2.last().unwrap().date_ms, &p2.last().unwrap().id)), 10).unwrap();
        assert_eq!(p3.len(), 5);
    }

    #[test]
    fn relabel_and_delete() {
        let mut s = Store::open_in_memory().unwrap();
        let mut m = msg(1);
        s.upsert_messages(&[m.clone()]).unwrap();
        m.labels = vec!["TRASH".into()];
        s.upsert_messages(&[m]).unwrap();
        assert_eq!(s.count("INBOX").unwrap(), 0);
        assert_eq!(s.count("TRASH").unwrap(), 1);
        s.delete_message("m00000001").unwrap();
        assert_eq!(s.count("TRASH").unwrap(), 0);
    }

    #[test]
    fn body_roundtrip_compressed() {
        let mut s = Store::open_in_memory().unwrap();
        let body = "<p>hello</p>".repeat(1000).into_bytes();
        s.set_body("m1", &body).unwrap();
        assert_eq!(s.body("m1").unwrap().unwrap(), body);
        assert!(s.body("nope").unwrap().is_none());
    }

    #[test]
    fn sync_state_roundtrip() {
        let s = Store::open_in_memory().unwrap();
        assert!(s.state("history_id").unwrap().is_none());
        s.set_state("history_id", "42").unwrap();
        assert_eq!(s.state("history_id").unwrap().as_deref(), Some("42"));
    }
}
