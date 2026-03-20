//! A JSON-RPC reverse proxy with a configurable request blocklist.

use axum::{
    Extension, Router, body::to_bytes, extract::Request, middleware, response::IntoResponse,
};
use axum_reverse_proxy::ReverseProxy;
use canhttp::http::json::JsonRpcRequest;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Clone, Debug)]
pub struct JsonRpcRequestMatcher {
    method: String,
}

impl JsonRpcRequestMatcher {
    pub fn with_method(method: impl Into<String>) -> Self {
        Self {
            method: method.into(),
        }
    }

    fn matches(&self, body: &[u8]) -> bool {
        serde_json::from_slice::<JsonRpcRequest<Value>>(body)
            .is_ok_and(|req| req.method() == self.method)
    }
}

type Blocklist = Arc<RwLock<Vec<JsonRpcRequestMatcher>>>;

pub struct JsonRpcReverseProxy {
    blocklist: Blocklist,
    port: u16,
}

impl JsonRpcReverseProxy {
    pub async fn start(target_url: &str, port: u16) -> Self {
        let blocklist: Blocklist = Default::default();
        let proxy: Router<()> = ReverseProxy::new("/", target_url).into();
        let app = proxy
            .layer(middleware::from_fn(block_middleware))
            .layer(Extension(blocklist.clone()));
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
            .await
            .unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.ok() });
        Self { blocklist, port }
    }

    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    pub async fn block(&self, filter: JsonRpcRequestMatcher) {
        self.blocklist.write().await.push(filter);
    }

    pub async fn clear_blocklist(&self) {
        self.blocklist.write().await.clear();
    }
}

async fn block_middleware(
    Extension(blocklist): Extension<Blocklist>,
    req: Request,
    next: middleware::Next,
) -> axum::response::Response {
    let blocklist = blocklist.read().await;
    if blocklist.is_empty() {
        return next.run(req).await;
    }

    let (parts, body) = req.into_parts();
    let bytes = to_bytes(body, 64 * 1024).await.unwrap();

    if blocklist.iter().any(|f| f.matches(&bytes)) {
        return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response();
    }

    next.run(Request::from_parts(parts, axum::body::Body::from(bytes)))
        .await
}
