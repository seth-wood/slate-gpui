# Slate

A fast, local-first, keyboard-driven Gmail client in Rust on [GPUI Kit](https://github.com/longbridge/gpui-kit)
(`gpui-kit` crate, built on Zed's GPUI). Plan: the "Slate: Rust Email Client Plan" doc in the project.

## Status: M0 (spike)

| Crate | What it does | Verified |
| --- | --- | --- |
| `slate-store` | SQLite (WAL) store, keyset-paged thread list, zstd bodies, sync state | unit tests; 100k messages: worst page 0.25 ms (`cargo run --release -p slate-store --example bench`) |
| `slate-mime` | MIME parse, HTML sanitize, render-tier classifier | unit tests; `--example tiers <dir of .eml>` measures tier mix on real mail |
| `slate-gmail` | Gmail REST client (list, metadata, history, modify), OAuth loopback + PKCE | unit tests against a mock server; not yet run against real Google |
| `slate-ui` (`slate` binary) | GPUI Kit window, virtual-list inbox over the store | renders in a virtual display; seeds a 100k-message demo store if none exists |

| `slate-sync` | Initial metadata sync + incremental sync from `history.list`, 404 falls back to full resync | unit tests against a mock server |
| `slate-view` | Locked-down web view for one message (sanitized HTML, CSP blocks scripts and network, links open in the browser) | renders a table-layout newsletter correctly; script and tracking pixel blocked |
| `slate-cli` | `login`, `sync`, `list`, `tiers`, `open` commands | builds; needs a real Google sign-in to exercise end to end |

Not verified yet: anything against real Google, and tier measurement on real mail.

## Sign in and sync your Gmail

Create a Google OAuth *Desktop app* client (Gmail API enabled, your account added as a test user), then:

    export SLATE_GOOGLE_CLIENT_ID=...apps.googleusercontent.com
    export SLATE_GOOGLE_CLIENT_SECRET=...        # never commit this
    cargo run -p slate-cli -- login              # opens the browser, saves a refresh token (0600)
    cargo run -p slate-cli -- sync               # newest 2000 inbox messages, then incremental on re-run
    cargo run -p slate-cli -- list 20
    cargo run -p slate-cli -- tiers 200          # how much of your mail needs the webview (counts only)
    cargo build                                  # builds slate, slate-cli and slate-view side by side
    cargo run -p slate-cli -- open <message-id>  # one message in the sandboxed web view
    cargo run -p slate-ui                        # window reads the synced mailbox; click a row to open it

## Run

    cargo test
    cargo run -p slate-ui            # SLATE_DB=path to use another store

Linux needs `libxkbcommon-dev`, `libxkbcommon-x11-dev`, X11/Wayland dev libs, and a Vulkan driver.
