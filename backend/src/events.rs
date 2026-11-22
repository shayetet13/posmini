use std::{convert::Infallible, time::Duration};

use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use futures_util::Stream;
use serde_json::Value;
use tokio::sync::broadcast::Sender;
use tokio_stream::{StreamExt, wrappers::BroadcastStream};

#[derive(Clone, Debug)]
pub struct Event {
    pub kind: &'static str,
    pub token: Option<String>,
    pub data: Value,
}

impl Event {
    pub fn new(kind: &'static str, data: Value) -> Self {
        Self {
            kind,
            token: None,
            data,
        }
    }

    pub fn for_order(kind: &'static str, token: &str, data: Value) -> Self {
        Self {
            kind,
            token: Some(token.to_string()),
            data,
        }
    }
}

pub fn stream(
    hub: Sender<Event>,
    keep: impl Fn(&Event) -> bool + Send + 'static,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let feed = BroadcastStream::new(hub.subscribe()).filter_map(move |item| match item {
        Ok(ev) if keep(&ev) => Some(Ok(SseEvent::default()
            .event(ev.kind)
            .data(ev.data.to_string()))),
        Ok(_) => None,
        Err(_lagged) => Some(Ok(SseEvent::default().event("refresh").data("{}"))),
    });
    Sse::new(feed).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}
