use crate::reqs::*;
use axum::{Json, extract::State};

pub async fn completions(
    State(state): State<AppState>,
    Json(payload): Json<IncReq>,
) -> axum::Json<Option<String>> {
    let res = state
        .client
        .post("http://0.0.0.0:8000/v1/chat/completions")
        .json(&payload)
        .send()
        .await
        .expect("Failed to send request");

    let data: OutRes = res.json().await.expect("Failed to parse JSON response");

    Json(data.choices[0].message.content.clone())
}

pub async fn metrics(State(state): State<AppState>) -> axum::Json<String> {
    let res = state
        .client
        .get("http://0.0.0.0:8001/metrics")
        .send()
        .await
        .expect("Failed to send request");

    let data: String = res.text().await.expect("Failed to parse text response");
    let lines: Vec<String> = data.lines().map(str::to_string).collect();
    Json(lines[1..5].join("---"))
}
