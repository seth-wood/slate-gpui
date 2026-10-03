//! `slate-view`: shows one email body in a locked-down web view.
//!
//! Usage: slate-view [--title TEXT] [file.html]   (reads stdin without a file)
//! The HTML is sanitized by slate-mime and carries a CSP that blocks scripts
//! and all network loads. Clicked http(s)/mailto links open in the default
//! browser; the view itself never navigates.

use std::io::Read;
use tao::{
    dpi::LogicalSize,
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    window::WindowBuilder,
};
use wry::WebViewBuilder;

fn open_external(url: &str) {
    if !(url.starts_with("http://") || url.starts_with("https://") || url.starts_with("mailto:")) {
        return;
    }
    let opener = if cfg!(target_os = "macos") { "open" } else if cfg!(windows) { "explorer" } else { "xdg-open" };
    let _ = std::process::Command::new(opener).arg(url).spawn();
}

fn main() {
    let mut title = String::from("Slate");
    let mut file = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--title" {
            title = args.next().unwrap_or_default();
        } else {
            file = Some(a);
        }
    }
    let html = match file {
        Some(f) => std::fs::read_to_string(f).expect("read html file"),
        None => {
            let mut s = String::new();
            std::io::stdin().read_to_string(&mut s).expect("read stdin");
            s
        }
    };
    let doc = slate_mime::webview_document(&html);

    let event_loop = EventLoop::new();
    let window = WindowBuilder::new().with_title(title).with_inner_size(LogicalSize::new(900.0, 700.0)).build(&event_loop).expect("window");

    let builder = WebViewBuilder::new()
        .with_html(doc)
        .with_navigation_handler(|url| {
            // Only the initial in-memory document may load; links go to the browser.
            if url.starts_with("about:") || url.starts_with("data:") {
                true
            } else {
                open_external(&url);
                false
            }
        })
        .with_new_window_req_handler(|url, _| {
            open_external(&url);
            wry::NewWindowResponse::Deny
        });

    #[cfg(target_os = "linux")]
    let _webview = {
        use tao::platform::unix::WindowExtUnix;
        use wry::WebViewBuilderExtUnix;
        let vbox = window.default_vbox().expect("gtk vbox");
        builder.build_gtk(vbox).expect("webview")
    };
    #[cfg(not(target_os = "linux"))]
    let _webview = builder.build(&window).expect("webview");

    event_loop.run(move |event, _, flow| {
        *flow = ControlFlow::Wait;
        if let Event::WindowEvent { event: WindowEvent::CloseRequested, .. } = event {
            *flow = ControlFlow::Exit;
        }
    });
}
