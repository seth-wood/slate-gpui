//! Seeds 100k messages and times the thread-list query the UI depends on.
use slate_store::{NewMessage, Store};
use std::time::Instant;

fn main() {
    let path = std::env::temp_dir().join("slate-bench.db");
    let _ = std::fs::remove_file(&path);
    let mut s = Store::open(&path).unwrap();
    let n = 100_000i64;
    let t = Instant::now();
    for chunk in (0..n).collect::<Vec<_>>().chunks(5_000) {
        let batch: Vec<_> = chunk.iter().map(|&i| NewMessage {
            id: format!("m{i:08}"), thread_id: format!("t{i}"), history_id: i as u64,
            date_ms: 1_700_000_000_000 + i * 60_000, from_name: format!("Sender {}", i % 500),
            from_addr: "s@example.com".into(), subject: format!("Subject line number {i}"),
            snippet: "A short preview of the message body goes here for the list row".into(),
            unread: i % 7 == 0, labels: vec!["INBOX".into()],
        }).collect();
        s.upsert_messages(&batch).unwrap();
    }
    println!("seed {n} messages: {:?}", t.elapsed());

    let mut worst = std::time::Duration::ZERO;
    let mut cursor: Option<(i64, String)> = None;
    let mut pages = 0;
    let t = Instant::now();
    loop {
        let q = Instant::now();
        let rows = s.page("INBOX", cursor.as_ref().map(|(d, i)| (*d, i.as_str())), 50).unwrap();
        worst = worst.max(q.elapsed());
        if rows.is_empty() { break; }
        let l = rows.last().unwrap();
        cursor = Some((l.date_ms, l.id.clone()));
        pages += 1;
    }
    println!("walked {pages} pages of 50: total {:?}, worst page {:?}", t.elapsed(), worst);
    let deep = Instant::now();
    let _ = s.page("INBOX", Some((1_700_000_000_000 + 50 * 60_000, "m00000050")), 50).unwrap();
    println!("deepest page: {:?}", deep.elapsed());
}
