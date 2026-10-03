//! `slate-cli`: sign in to Gmail, sync the inbox into the local store, list it.
//!
//! Google OAuth client credentials come from the environment and are never
//! stored in the repo:
//!   SLATE_GOOGLE_CLIENT_ID, SLATE_GOOGLE_CLIENT_SECRET
//! The refresh token is saved to ~/.config/slate/tokens.json (mode 0600).
//! TODO: move it to the OS keychain.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use slate_gmail::{Client, auth};
use slate_store::Store;
use std::path::PathBuf;

#[derive(Serialize, Deserialize)]
struct Saved {
    refresh_token: String,
}

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| {
        eprintln!("missing {name}; set SLATE_GOOGLE_CLIENT_ID and SLATE_GOOGLE_CLIENT_SECRET");
        std::process::exit(2)
    })
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").expect("HOME not set"))
}
fn token_path() -> PathBuf {
    std::env::var("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|_| home().join(".config")).join("slate/tokens.json")
}
fn db_path() -> PathBuf {
    std::env::var("SLATE_DB").map(PathBuf::from).unwrap_or_else(|_| {
        std::env::var("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|_| home().join(".local/share")).join("slate/slate.db")
    })
}

fn save_token(refresh: &str) {
    let p = token_path();
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, serde_json::to_vec(&Saved { refresh_token: refresh.into() }).unwrap()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
}

async fn login() {
    let (id, secret) = (env("SLATE_GOOGLE_CLIENT_ID"), env("SLATE_GOOGLE_CLIENT_SECRET"));
    let pkce = auth::Pkce::new();
    let mut st = [0u8; 16];
    rand::rng().fill_bytes(&mut st);
    let state = B64.encode(st);
    let lb = auth::Loopback::bind().await.expect("bind loopback port");
    let redirect = lb.redirect_uri();
    let url = auth::authorize_url(&id, &redirect, &pkce, &state);
    println!("Open this URL in your browser to sign in:\n\n{url}\n");
    for opener in ["xdg-open", "open"] {
        if std::process::Command::new(opener).arg(&url).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().is_ok() {
            break;
        }
    }
    let code = lb.wait_for_code(&state).await.expect("sign-in failed");
    let t = auth::exchange_code(auth::TOKEN_URL, &id, &secret, &code, &redirect, &pkce.verifier).await.expect("token exchange failed");
    let Some(refresh) = t.refresh_token else {
        eprintln!("Google returned no refresh token; remove Slate at myaccount.google.com/permissions and retry");
        std::process::exit(1)
    };
    save_token(&refresh);
    let me = Client::new(t.access_token).profile().await.expect("profile");
    println!("Signed in as {}. Token saved to {}", me.email_address, token_path().display());
}

async fn client() -> Client {
    let (id, secret) = (env("SLATE_GOOGLE_CLIENT_ID"), env("SLATE_GOOGLE_CLIENT_SECRET"));
    let saved: Saved = serde_json::from_slice(&std::fs::read(token_path()).unwrap_or_else(|_| {
        eprintln!("not signed in; run `slate-cli login` first");
        std::process::exit(2)
    }))
    .expect("corrupt tokens.json");
    let t = auth::refresh(auth::TOKEN_URL, &id, &secret, &saved.refresh_token).await.expect("token refresh failed; run `slate-cli login` again");
    Client::new(t.access_token)
}

fn open_store() -> Store {
    let p = db_path();
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    Store::open(p).expect("open store")
}

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("login") => login().await,
        Some("sync") => {
            let limit = args.next().and_then(|a| a.parse().ok()).unwrap_or(2000);
            let c = client().await;
            let mut store = open_store();
            let t = std::time::Instant::now();
            let s = slate_sync::sync(&c, &mut store, "INBOX", limit).await.expect("sync failed");
            println!("{} in {:?}: {} upserted, {} deleted", if s.full_resync { "initial sync" } else { "incremental sync" }, t.elapsed(), s.upserted, s.deleted);
        }
        Some("list") => {
            let n = args.next().and_then(|a| a.parse().ok()).unwrap_or(20);
            for r in open_store().page("INBOX", None, n).expect("query") {
                println!("{} {:<24.24} {}", if r.unread { "*" } else { " " }, r.from_name, r.subject);
            }
        }
        _ => eprintln!("usage: slate-cli login | sync [limit] | list [n]"),
    }
}
