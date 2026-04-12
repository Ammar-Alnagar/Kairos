use axum::{
    Router,
    routing::{get, post},
};
use dashmap::DashMap;
use reqwest::Client;
use std::sync::Arc;
use tokio::net::TcpListener;

pub mod func;
pub mod reqs;
#[cfg(test)]
mod tests;

use crate::func::{completions, metrics, scrape_metrics_loop};
use crate::reqs::AppState;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let endpoints: Vec<String> = std::env::var("ENDPOINTS")
        .unwrap_or_else(|_| "http://0.0.0.0:8000,http://0.0.0.0:8001".to_string())
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    tracing::info!("Kairos starting — endpoints: {:?}", endpoints);

    let state = AppState {
        client: Arc::new(Client::new()),
        endpoints,
        scores: Arc::new(DashMap::new()),
        top: Arc::new(DashMap::new()),
    };

    // Start background metrics scraper
    tokio::spawn(scrape_metrics_loop(state.clone()));

    let app = Router::new()
        .route("/", get(|| async { "Kairos — LLM load balancer" }))
        .route("/v1/chat", get(metrics))
        .route("/v1/chat/completions", post(completions))
        .with_state(state);

    let port = std::env::var("PORT").unwrap_or_else(|_| "3000".to_string());
    let addr = format!("0.0.0.0:{}", port);
    let listener = TcpListener::bind(&addr).await.unwrap();
    tracing::info!("listening on {}", addr);
    axum::serve(listener, app).await.unwrap();
}
