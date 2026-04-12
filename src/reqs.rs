use dashmap::DashMap;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// Application state
#[derive(Clone)]
pub struct AppState {
    pub client: Arc<Client>,
    pub endpoints: Vec<String>,
    pub scores: Arc<DashMap<String, f64>>,
    pub top: Arc<DashMap<String, String>>,
}

// Incoming request

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct InMessages {
    pub role: String,
    pub content: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct IncReq {
    pub model: String,
    pub messages: Vec<InMessages>,
    pub max_tokens: usize,
    pub temperature: f64,
    pub top_p: f32,
    pub stream: Option<bool>,
}

// Upstream response types

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Logprobs {
    pub content: Option<serde_json::Value>,
    pub refusal: Option<serde_json::Value>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ResMessage {
    pub content: Option<String>,
    pub refusal: Option<String>,
    pub role: String,
    pub tool_calls: Option<serde_json::Value>,
    pub function_call: Option<serde_json::Value>,
    pub reasoning: Option<serde_json::Value>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Choice {
    pub finish_reason: String,
    pub index: u8,
    pub logprobs: Option<Logprobs>,
    pub message: ResMessage,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Usage {
    pub completion_tokens: usize,
    pub prompt_tokens: usize,
    pub total_tokens: usize,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct OutRes {
    pub choices: Vec<Choice>,
    pub created: usize,
    pub id: String,
    pub model: String,
    pub object: String,
    pub service_tier: Option<serde_json::Value>,
    pub system_fingerprint: Option<serde_json::Value>,
    pub usage: Usage,
}
