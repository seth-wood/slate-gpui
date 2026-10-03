//! M0 spike: a GPUI Kit window showing the inbox from the local store
//! through a virtual list. With no mailbox on disk it seeds a 100k-message
//! demo store so scrolling performance can be judged right away.

use gpui_kit::component::*;
use gpui_kit::*;
use gpui_kit::gpui::{Size, prelude::FluentBuilder as _};
use slate_store::{NewMessage, Row, Store};
use std::rc::Rc;

const ROW_H: f32 = 64.0;
const LOAD: usize = 5_000;

struct ThreadList {
    rows: Vec<Row>,
    sizes: Rc<Vec<Size<Pixels>>>,
    width: Pixels,
}

impl ThreadList {
    fn new(rows: Vec<Row>) -> Self {
        let sizes = Rc::new(rows.iter().map(|_| size(px(1.0), px(ROW_H))).collect());
        Self { rows, sizes, width: px(800.0) }
    }
}

impl Render for ThreadList {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.width = window.viewport_size().width;
        let view = cx.entity();
        div().size_full().child(v_virtual_list(view, "threads", self.sizes.clone(), |this, range, _, _| {
            range
                .map(|i| {
                    let r = &this.rows[i];
                    div()
                        .id(i)
                        .v_flex()
                        .w(this.width)
                        .h(px(ROW_H))
                        .overflow_hidden()
                        .text_sm()
                        .line_height(px(18.0))
                        .px_3()
                        .py_1()
                                                .border_b_1()
                        .child(
                            div()
                                .h_flex()
                                .justify_between()
                                .child(div().when(r.unread, |d| d.font_semibold()).child(r.from_name.clone()))
                                .child(div().text_xs().child(format_age(r.date_ms))),
                        )
                        .child(div().when(r.unread, |d| d.font_semibold()).child(r.subject.clone()))
                        .child(div().text_xs().child(r.snippet.clone()))
                })
                .collect()
        }))
    }
}

fn format_age(date_ms: i64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(date_ms);
    let mins = ((now - date_ms) / 60_000).max(0);
    match mins {
        0..=59 => format!("{mins}m"),
        60..=1439 => format!("{}h", mins / 60),
        _ => format!("{}d", mins / 1440),
    }
}

fn seed_demo(store: &mut Store) {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64;
    for chunk in (0..100_000i64).collect::<Vec<_>>().chunks(5_000) {
        let batch: Vec<_> = chunk
            .iter()
            .map(|&i| NewMessage {
                id: format!("m{i:08}"),
                thread_id: format!("t{i}"),
                history_id: i as u64,
                date_ms: now - i * 60_000,
                from_name: format!("Sender {}", i % 500),
                from_addr: "s@example.com".into(),
                subject: format!("Subject line number {i}"),
                snippet: "A short preview of the message body goes here for the list row".into(),
                unread: i % 7 == 0,
                labels: vec!["INBOX".into()],
            })
            .collect();
        store.upsert_messages(&batch).unwrap();
    }
}

fn main() {
    let path = std::env::var("SLATE_DB").unwrap_or_else(|_| std::env::temp_dir().join("slate-demo.db").to_string_lossy().into());
    let mut store = Store::open(&path).expect("open store");
    if store.count("INBOX").unwrap_or(0) == 0 {
        seed_demo(&mut store);
    }
    let rows = store.page("INBOX", None, LOAD).expect("load inbox");

    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        gpui_kit::open_window(WindowOptions::default(), cx, move |_, cx| cx.new(|_| ThreadList::new(rows)))
            .expect("open window");
    });
}
