use crate::reqs::*;
use async_stream::stream;
use axum::{
    Json,
    body::Body,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use bytes::Bytes;

use std::{io, time::Duration};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const CHUNK_IDLE_TIMEOUT: Duration = Duration::from_secs(30);
const SCRAPE_INTERVAL: Duration = Duration::from_secs(5);

// Background metrics scraper
pub async fn scrape_metrics_loop(state: AppState) {
    let mut interval = tokio::time::interval(SCRAPE_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        interval.tick().await;

        // Fire all metric fetches concurrently
        let mut handles = Vec::with_capacity(state.endpoints.len());
        for endpoint in &state.endpoints {
            let client = state.client.clone();
            let url = endpoint.clone();
            handles.push(tokio::spawn(async move {
                let score = fetch_score(&client, &url).await;
                (url, score)
            }));
        }

        for handle in handles {
            if let Ok((url, score)) = handle.await {
                tracing::info!(
                    "[scraper] {} -> {}",
                    url,
                    if score < f64::MAX {
                        format!("{:.2}", score)
                    } else {
                        "unreachable".into()
                    }
                );
                state.scores.insert(url, score);
            }
        }

        // Select endpoint with lowest reachable score
        let best = state
            .endpoints
            .iter()
            .filter_map(|u| state.scores.get(u).map(|s| (u.clone(), *s)))
            .filter(|(_, s)| *s < f64::MAX)
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

        match best {
            Some((url, score)) => {
                tracing::info!("[scraper] top -> {} (score {:.2})", url, score);
                state.top.insert("top".to_string(), url);
            }
            None => {
                tracing::warn!("[scraper] all endpoints unreachable — clearing top");
                state.top.remove("top");
            }
        }
    }
}

// Fetch /metrics and compute load score
async fn fetch_score(client: &reqwest::Client, base: &str) -> f64 {
    let url = format!("{}/metrics", base);
    match tokio::time::timeout(CONNECT_TIMEOUT, client.get(&url).send()).await {
        Ok(Ok(r)) if r.status().is_success() => r
            .text()
            .await
            .map(|t| parse_load_score(&t))
            .unwrap_or(f64::MAX),
        _ => f64::MAX,
    }
}

// Parse vLLM metrics: score = running + waiting * 2
pub(crate) fn parse_load_score(text: &str) -> f64 {
    let mut running = 0.0_f64;
    let mut waiting = 0.0_f64;

    for line in text.lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if line.starts_with("vllm:num_requests_running") {
            running = line
                .split_whitespace()
                .last()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.0);
        } else if line.starts_with("vllm:num_requests_waiting") {
            waiting = line
                .split_whitespace()
                .last()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.0);
        }
    }

    running + waiting * 2.0
}

// Sort endpoints by score, top entry first
pub(crate) fn ordered_endpoints(state: &AppState) -> Vec<String> {
    let top = state.top.get("top").map(|v| v.clone());

    let mut pairs: Vec<(String, f64)> = state
        .endpoints
        .iter()
        .map(|u| {
            (
                u.clone(),
                state.scores.get(u).map(|s| *s).unwrap_or(f64::MAX),
            )
        })
        .collect();

    pairs.sort_by(|a, b| match &top {
        Some(t) if t == &a.0 => std::cmp::Ordering::Less,
        Some(t) if t == &b.0 => std::cmp::Ordering::Greater,
        _ => a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal),
    });

    pairs.into_iter().map(|(u, _)| u).collect()
}

// Handlers

// POST /v1/chat/completions - proxy with fallback chain
pub async fn completions(State(state): State<AppState>, Json(payload): Json<IncReq>) -> Response {
    let is_stream = payload.stream.unwrap_or(false);
    let endpoints = ordered_endpoints(&state);

    for endpoint in &endpoints {
        let url = format!("{}/v1/chat/completions", endpoint);
        tracing::debug!("[proxy] trying {}", url);

        // Attempt connection with a hard timeout so a hanging endpoint never
        // blocks the fallback chain.
        let res = match tokio::time::timeout(
            CONNECT_TIMEOUT,
            state.client.post(&url).json(&payload).send(),
        )
        .await
        {
            Ok(Ok(r)) if r.status().is_success() => r,
            Ok(Ok(r)) => {
                tracing::warn!("[proxy] {} -> HTTP {}", endpoint, r.status());
                continue;
            }
            Ok(Err(e)) => {
                tracing::warn!("[proxy] {} -> send error: {}", endpoint, e);
                continue;
            }
            Err(_) => {
                tracing::warn!("[proxy] {} -> connect timeout", endpoint);
                continue;
            }
        };

        if is_stream {
            // Streaming: forward SSE with idle timeout
            let s = stream! {
                let mut res = res;
                loop {
                    match tokio::time::timeout(CHUNK_IDLE_TIMEOUT, res.chunk()).await {
                        // Forward chunk
                        Ok(Ok(Some(chunk))) => yield Ok::<Bytes, io::Error>(chunk),

                        // Upstream finished
                        Ok(Ok(None)) => break,

                        // Upstream error
                        Ok(Err(e)) => {
                            tracing::warn!("[stream] upstream error: {}", e);
                            let msg = format!(
                                "data: {{\"error\":\"upstream_error\",\"detail\":\"{e}\"}}\n\ndata: [DONE]\n\n"
                            );
                            yield Ok(Bytes::from(msg));
                            break;
                        }

                        // Backend stalled
                        Err(_elapsed) => {
                            tracing::warn!("[stream] upstream stalled — closing stream");
                            yield Ok(Bytes::from(
                                "data: {\"error\":\"upstream_stalled\"}\n\ndata: [DONE]\n\n",
                            ));
                            break;
                        }
                    }
                }
            };

            return axum::http::Response::builder()
                .status(StatusCode::OK)
                .header("content-type", "text/event-stream; charset=utf-8")
                .header("cache-control", "no-cache")
                .header("x-accel-buffering", "no")
                .body(Body::from_stream(s))
                .unwrap();
        } else {
            // Non-streaming: forward full JSON response
            match res.bytes().await {
                Ok(bytes) => {
                    return axum::http::Response::builder()
                        .status(StatusCode::OK)
                        .header("content-type", "application/json")
                        .body(Body::from(bytes))
                        .unwrap();
                }
                Err(e) => {
                    tracing::warn!("[proxy] {} -> body read error: {}", endpoint, e);
                    continue;
                }
            }
        }
    }

    // Every backend failed
    (StatusCode::BAD_GATEWAY, "all backends unavailable").into_response()
}

// GET /v1/chat - endpoint status snapshot
pub async fn metrics(State(state): State<AppState>) -> impl IntoResponse {
    let top = state.top.get("top").map(|v| v.clone()).unwrap_or_default();

    let report: Vec<serde_json::Value> = state
        .endpoints
        .iter()
        .map(|url| {
            let score = state.scores.get(url).map(|s| *s);
            let status = match score {
                None => "pending",
                Some(s) if s >= f64::MAX => "unreachable",
                Some(_) => "ok",
            };
            serde_json::json!({
                "endpoint": url,
                "score":    score.filter(|&s| s < f64::MAX),
                "status":   status,
                "top":      url == &top,
            })
        })
        .collect();

    Json(report)
}
