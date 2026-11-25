//! Event streams carried over a WebSocket, so the window's streams stop using up its HTTP
//! connections.
//!
//! WebKit opens at most six HTTP/1.1 connections to one host and port, and an SSE stream holds one
//! for as long as it lives: `/api/events` per project, plus a chat stream and a subagents stream for
//! each lane the window has opened. Three opened lanes reached six. After that, every command the
//! window sent to that daemon (a send, a trust save, a review refresh) waited in WebKit's queue
//! behind streams that never end, while the daemon kept answering every other client in under a
//! millisecond. WebSockets are not counted against that limit (the terminal's socket never was),
//! so the page opens each stream as `/api/stream?path=<the stream's own path and query>`. This
//! handler runs that request through the same router, guard and failure report included, and
//! forwards the response body. The first frame is the status as text. After it, the body goes as
//! binary frames, so a multi-byte character split across two chunks is decoded whole by the page.

use axum::{
    Router,
    body::Body,
    extract::{
        ConnectInfo, Query, Request,
        ws::{Message, WebSocketUpgrade},
    },
    http::{HeaderMap, StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
use futures_util::StreamExt;
use std::{net::SocketAddr, sync::OnceLock};
use tower_service::Service;

/// The daemon's own router, set once at startup so a tunnelled request takes exactly the path a
/// direct one would.
static ROUTER: OnceLock<Router> = OnceLock::new();

/// Hands the tunnel the router it forwards into. Called once from `serve`.
pub fn install(router: Router) {
    let _ = ROUTER.set(router);
}

#[derive(serde::Deserialize)]
pub struct StreamQuery {
    /// The stream's path and query, e.g. `/api/chat/events?lane=…`.
    path: String,
}

/// The request to forward, if `path` names a stream on this daemon and not the tunnel itself.
fn target(path: &str) -> Option<Uri> {
    let uri: Uri = path.parse().ok()?;
    let p = uri.path();
    (uri.scheme().is_none()
        && uri.authority().is_none()
        && p.starts_with("/api/")
        && !p.starts_with("/api/stream"))
    .then_some(uri)
}

/// Builds the inner request. It carries the socket's own headers, including the `keel.<token>`
/// subprotocol that `pair::app_token` reads, minus the ones that describe the upgrade.
fn inner(uri: Uri, mut headers: HeaderMap, peer: SocketAddr) -> Request {
    for h in [
        header::UPGRADE,
        header::CONNECTION,
        header::SEC_WEBSOCKET_KEY,
        header::SEC_WEBSOCKET_VERSION,
        header::SEC_WEBSOCKET_EXTENSIONS,
    ] {
        headers.remove(h);
    }
    headers.insert(
        header::ACCEPT,
        header::HeaderValue::from_static("text/event-stream"),
    );
    let mut req = Request::new(Body::empty());
    *req.uri_mut() = uri;
    *req.headers_mut() = headers;
    req.extensions_mut().insert(ConnectInfo(peer));
    req
}

/// `GET /api/stream?path=…`: one SSE stream, over a WebSocket.
pub async fn ws(
    ws: WebSocketUpgrade,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Query(q): Query<StreamQuery>,
    headers: HeaderMap,
) -> Response {
    // no-blocking: an upgrade; the forwarded handler keeps its own blocking work off the executor.
    let Some(uri) = target(&q.path) else {
        return (StatusCode::BAD_REQUEST, "not a stream on this daemon").into_response();
    };
    let req = inner(uri, headers, peer);
    ws.protocols(["keel"])
        .on_upgrade(move |mut socket| async move {
            let Some(router) = ROUTER.get() else { return };
            let Ok(response) = router.clone().call(req).await;
            let status = response.status().as_u16().to_string();
            if socket.send(Message::Text(status.into())).await.is_err() {
                return;
            }
            let mut body = response.into_body().into_data_stream();
            // Dropping `body` when the page closes is what a dropped TCP connection did before: the
            // SSE handler's stream ends and whatever it subscribed to is let go.
            loop {
                tokio::select! {
                    chunk = body.next() => match chunk {
                        Some(Ok(bytes)) => {
                            if socket.send(Message::Binary(bytes)).await.is_err() { break; }
                        }
                        _ => break,
                    },
                    incoming = socket.recv() => match incoming {
                        Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                        _ => {}
                    },
                }
            }
            let _ = socket.send(Message::Close(None)).await;
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_this_daemons_api_is_forwarded() {
        assert!(target("/api/chat/events?lane=a&after=3").is_some());
        assert!(target("/api/events").is_some());
        assert!(target("/api/stream?path=/api/events").is_none());
        assert!(target("http://evil.example/api/events").is_none());
        assert!(target("//evil.example/api/events").is_none());
        assert!(target("/other").is_none());
    }

    #[test]
    fn the_inner_request_keeps_the_token_and_drops_the_upgrade() {
        let mut h = HeaderMap::new();
        h.insert(header::UPGRADE, "websocket".parse().unwrap());
        h.insert(
            header::SEC_WEBSOCKET_PROTOCOL,
            "keel, keel.t".parse().unwrap(),
        );
        h.insert(header::ORIGIN, "tauri://localhost".parse().unwrap());
        let peer: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let req = inner("/api/events".parse().unwrap(), h, peer);
        assert!(req.headers().get(header::UPGRADE).is_none());
        assert_eq!(
            req.headers()[header::SEC_WEBSOCKET_PROTOCOL],
            "keel, keel.t"
        );
        assert_eq!(req.headers()[header::ORIGIN], "tauri://localhost");
        assert!(req.extensions().get::<ConnectInfo<SocketAddr>>().is_some());
    }
}
