//! Usage: cargo run -p slate-mime --example tiers -- <dir-of-.eml-files>
//! M0 measurement: what share of a real mailbox each render tier handles.
use slate_mime::{Tier, classify, parse};
use std::collections::HashMap;

fn main() {
    let dir = std::env::args().nth(1).expect("usage: tiers <dir of .eml files>");
    let mut counts: HashMap<Tier, usize> = HashMap::new();
    let mut total = 0;
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let Ok(raw) = std::fs::read(e.path()) else { continue };
        let Some(p) = parse(&raw) else { continue };
        *counts.entry(classify(p.html.as_deref())).or_default() += 1;
        total += 1;
    }
    for (t, n) in [Tier::Text, Tier::Native, Tier::Webview].map(|t| (t, counts.get(&t).copied().unwrap_or(0))) {
        println!("{t:?}: {n} ({:.1}%)", 100.0 * n as f64 / total.max(1) as f64);
    }
    println!("total: {total}");
}
