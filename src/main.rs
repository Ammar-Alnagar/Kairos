use axum::{
    Router,
    routing::{get, post},
};
use reqwest::Client;
use tokio::net::TcpListener;
pub mod func;
pub mod reqs;
use crate::func::*;
use crate::reqs::*;

#[tokio::main]
async fn main() {
    let client = Client::new();
    let state = AppState { client };

    let app = Router::new()
        .route("/", get(|| async { "welcome to the root" }))
        .route("/v1/chat", get(metrics))
        .route("/v1/chat/completions", post(completions))
        .with_state(state);

    let listener = TcpListener::bind("0.0.0.0:3000").await.unwrap();
    println!("listening on port 3000");
    axum::serve(listener, app).await.unwrap();
}
