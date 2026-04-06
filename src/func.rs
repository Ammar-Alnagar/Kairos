use crate::reqs::*;
use axum::Json;
use reqwest::Client;

pub async fn completions(Json(payload): Json<IncReq>) -> axum::Json<Option<String>> {
    // let client = Client::new();

    let res = client
        .post("http://0.0.0.0:8000/v1/chat/completions")
        .json(&payload)
        .send()
        .await
        .expect("Failed to send request");

    let data: OutRes = res.json().await.expect("Failed to parse JSON response");

    Json(data.choices[0].message.content.clone())
}
