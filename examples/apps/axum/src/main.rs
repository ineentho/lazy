use axum::{routing::get, Json, Router};
use serde_json::json;
use std::{env, net::SocketAddr};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() {
    let port = env::var("PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(3000);
    let host = env::var("HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let addr: SocketAddr = format!("{host}:{port}").parse().expect("valid bind addr");

    let app = Router::new().route("/", get(hello));
    let listener = TcpListener::bind(addr).await.expect("bind listener");
    axum::serve(listener, app).await.expect("serve axum app");
}

async fn hello() -> Json<serde_json::Value> {
    Json(json!({
        "app": "axum",
        "message": "Hello from Axum",
        "lazyUrl": env::var("LAZY_URL").unwrap_or_default()
    }))
}
