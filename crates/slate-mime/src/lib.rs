//! MIME parsing, HTML sanitizing, and the render-tier classifier.
//!
//! M0 question: how much real mail can the native GPUI renderer (Tier 1)
//! handle, and how much needs the sandboxed webview (Tier 2)? `classify`
//! answers that per message so we can measure it on a real inbox.

use mail_parser::MessageParser;
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    pub subject: String,
    pub from_name: String,
    pub from_addr: String,
    pub text: Option<String>,
    pub html: Option<String>,
}

pub fn parse(raw: &[u8]) -> Option<Parsed> {
    let m = MessageParser::default().parse(raw)?;
    let from = m.from().and_then(|a| a.first());
    Some(Parsed {
        subject: m.subject().unwrap_or("").to_string(),
        from_name: from.and_then(|a| a.name()).unwrap_or("").to_string(),
        from_addr: from.and_then(|a| a.address()).unwrap_or("").to_string(),
        text: m.body_text(0).map(|s| s.into_owned()),
        html: m.body_html(0).map(|s| s.into_owned()),
    })
}

/// Strip scripts, event handlers, forms, remote-loading tags and anything
/// else that must never reach a renderer. Remote images are removed too, so
/// tracking pixels never fire until the user opts in.
pub fn sanitize(html: &str) -> String {
    let mut b = ammonia::Builder::default();
    b.rm_tags(["img", "form", "input", "iframe", "object", "embed", "link", "style", "script"]);
    b.link_rel(Some("noopener noreferrer"));
    b.clean(html).to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tier {
    /// Plain text or no HTML: render as native text.
    Text,
    /// Simple HTML (paragraphs, links, lists, emphasis, quotes): native rich text.
    Native,
    /// Layout-dependent HTML (tables, inline CSS layout, images-as-content): webview.
    Webview,
}

/// Decide the render tier for an HTML body. Tables used for layout, `<style>`
/// blocks, and positioning attributes are what native renderers get wrong.
pub fn classify(html: Option<&str>) -> Tier {
    let Some(h) = html else { return Tier::Text };
    let l = h.to_ascii_lowercase();
    let layout = ["<table", "<style", "<center", "<font", "float:", "position:", "background-image", "width=\"6", "cellpadding"];
    if layout.iter().any(|t| l.contains(t)) {
        return Tier::Webview;
    }
    let allowed: HashSet<&str> = ["p","br","a","b","strong","i","em","u","ul","ol","li","blockquote","pre","code","div","span","h1","h2","h3","h4","hr","html","body","head","meta","title"].into_iter().collect();
    let mut rest = l.as_str();
    while let Some(i) = rest.find('<') {
        rest = &rest[i + 1..];
        let end = rest.find(|c: char| !(c.is_ascii_alphanumeric())).unwrap_or(rest.len());
        let name = rest[..end].trim_start_matches('/');
        if !name.is_empty() && !name.starts_with('!') && !allowed.contains(name) {
            return Tier::Webview;
        }
    }
    Tier::Native
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIMPLE: &[u8] = b"From: Ada Lovelace <ada@example.com>\r\nSubject: Hi\r\nContent-Type: text/plain\r\n\r\nHello there\r\n";
    const HTML: &[u8] = b"From: News <n@example.com>\r\nSubject: Deal\r\nContent-Type: text/html\r\n\r\n<table><tr><td>Hi</td></tr></table>\r\n";

    #[test]
    fn parses_headers_and_text() {
        let p = parse(SIMPLE).unwrap();
        assert_eq!(p.subject, "Hi");
        assert_eq!(p.from_name, "Ada Lovelace");
        assert_eq!(p.from_addr, "ada@example.com");
        assert_eq!(p.text.as_deref().map(str::trim), Some("Hello there"));
    }

    #[test]
    fn sanitize_removes_scripts_handlers_and_images() {
        let out = sanitize(r#"<p onclick="x()">Hi</p><script>alert(1)</script><img src="http://t.example/p.gif"><a href="https://a.example">l</a>"#);
        assert!(!out.contains("script") && !out.contains("onclick") && !out.contains("<img"));
        assert!(out.contains("<a ") && out.contains("Hi"));
    }

    #[test]
    fn tiers() {
        assert_eq!(classify(None), Tier::Text);
        assert_eq!(classify(Some("<p>Hi <b>there</b> <a href=x>link</a></p>")), Tier::Native);
        assert_eq!(classify(Some("<table><tr><td>x</td></tr></table>")), Tier::Webview);
        assert_eq!(classify(Some("<p>x</p><style>p{}</style>")), Tier::Webview);
        assert_eq!(classify(Some("<p>x</p><img src=a>")), Tier::Webview);
        let p = parse(HTML).unwrap();
        assert_eq!(classify(p.html.as_deref()), Tier::Webview);
    }
}
