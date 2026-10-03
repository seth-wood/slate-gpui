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

Not done yet (needs a Google OAuth client): real login, real inbox sync, tier measurement on real mail.

## Run

    cargo test
    cargo run -p slate-ui            # SLATE_DB=path to use another store

Linux needs `libxkbcommon-dev`, `libxkbcommon-x11-dev`, X11/Wayland dev libs, and a Vulkan driver.
