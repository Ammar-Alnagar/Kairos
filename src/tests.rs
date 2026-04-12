#[cfg(test)]
mod unit {
    use crate::func::{ordered_endpoints, parse_load_score};
    use crate::reqs::AppState;
    use dashmap::DashMap;
    use reqwest::Client;
    use std::sync::Arc;

    fn test_state(endpoints: Vec<&str>) -> AppState {
        AppState {
            client: Arc::new(Client::new()),
            endpoints: endpoints.iter().map(|s| s.to_string()).collect(),
            scores: Arc::new(DashMap::new()),
            top: Arc::new(DashMap::new()),
        }
    }

    // ── parse_load_score tests ──────────────────────────────────────────────

    #[test]
    fn test_parse_score_empty() {
        assert_eq!(parse_load_score(""), 0.0);
    }

    #[test]
    fn test_parse_score_only_comments() {
        let input = "# HELP vllm metric\n# TYPE vllm gauge\n";
        assert_eq!(parse_load_score(input), 0.0);
    }

    #[test]
    fn test_parse_score_zero_load() {
        let input = "vllm:num_requests_running 0\nvllm:num_requests_waiting 0\n";
        assert_eq!(parse_load_score(input), 0.0);
    }

    #[test]
    fn test_parse_score_running_only() {
        let input = "vllm:num_requests_running 5\nvllm:num_requests_waiting 0\n";
        assert_eq!(parse_load_score(input), 5.0);
    }

    #[test]
    fn test_parse_score_waiting_only() {
        let input = "vllm:num_requests_running 0\nvllm:num_requests_waiting 3\n";
        assert_eq!(parse_load_score(input), 6.0); // waiting * 2
    }

    #[test]
    fn test_parse_score_mixed() {
        let input = "vllm:num_requests_running 4\nvllm:num_requests_waiting 2\n";
        assert_eq!(parse_load_score(input), 8.0); // 4 + 2*2
    }

    #[test]
    fn test_parse_score_with_extra_metrics() {
        let input =
            "vllm:num_requests_running 3\nvllm:other_metric 100\nvllm:num_requests_waiting 1\n";
        assert_eq!(parse_load_score(input), 5.0); // 3 + 1*2
    }

    #[test]
    fn test_parse_score_float_values() {
        let input = "vllm:num_requests_running 2.5\nvllm:num_requests_waiting 1.5\n";
        assert_eq!(parse_load_score(input), 5.5); // 2.5 + 1.5*2
    }

    #[test]
    fn test_parse_score_malformed_fallback() {
        let input = "vllm:num_requests_running abc\nvllm:num_requests_waiting xyz\n";
        assert_eq!(parse_load_score(input), 0.0); // falls back to 0.0
    }

    // ── ordered_endpoints tests ─────────────────────────────────────────────

    #[test]
    fn test_ordering_no_scores() {
        let state = test_state(vec!["http://a:8000", "http://b:8000"]);
        let ordered = ordered_endpoints(&state);
        assert_eq!(ordered.len(), 2);
        // All scores are MAX, order is undefined but stable
    }

    #[test]
    fn test_ordering_by_score() {
        let state = test_state(vec!["http://a:8000", "http://b:8000", "http://c:8000"]);
        state.scores.insert("http://a:8000".into(), 10.0);
        state.scores.insert("http://b:8000".into(), 2.0);
        state.scores.insert("http://c:8000".into(), 5.0);

        let ordered = ordered_endpoints(&state);
        assert_eq!(ordered[0], "http://b:8000");
        assert_eq!(ordered[1], "http://c:8000");
        assert_eq!(ordered[2], "http://a:8000");
    }

    #[test]
    fn test_ordering_top_entry_first() {
        let state = test_state(vec!["http://a:8000", "http://b:8000", "http://c:8000"]);
        state.scores.insert("http://a:8000".into(), 1.0);
        state.scores.insert("http://b:8000".into(), 2.0);
        state.scores.insert("http://c:8000".into(), 3.0);
        state.top.insert("top".into(), "http://c:8000".into());

        let ordered = ordered_endpoints(&state);
        assert_eq!(ordered[0], "http://c:8000"); // top forced first
        assert_eq!(ordered[1], "http://a:8000");
        assert_eq!(ordered[2], "http://b:8000");
    }

    #[test]
    fn test_ordering_with_unreachable() {
        let state = test_state(vec!["http://a:8000", "http://b:8000", "http://c:8000"]);
        state.scores.insert("http://a:8000".into(), f64::MAX);
        state.scores.insert("http://b:8000".into(), 1.0);
        state.scores.insert("http://c:8000".into(), f64::MAX);

        let ordered = ordered_endpoints(&state);
        assert_eq!(ordered[0], "http://b:8000");
    }

    #[test]
    fn test_ordering_single_endpoint() {
        let state = test_state(vec!["http://a:8000"]);
        state.scores.insert("http://a:8000".into(), 5.0);
        let ordered = ordered_endpoints(&state);
        assert_eq!(ordered, vec!["http://a:8000"]);
    }
}

#[cfg(test)]
mod integration {
    use axum::http::{Request, StatusCode, header};
    use axum::{
        Router,
        body::Body,
        routing::{get, post},
    };
    use dashmap::DashMap;
    use reqwest::Client;
    use serde_json::json;
    use std::sync::Arc;
    use tower::ServiceExt;

    use crate::func::{completions, metrics};
    use crate::reqs::AppState;

    fn test_state(endpoints: Vec<&str>) -> AppState {
        AppState {
            client: Arc::new(Client::new()),
            endpoints: endpoints.iter().map(|s| s.to_string()).collect(),
            scores: Arc::new(DashMap::new()),
            top: Arc::new(DashMap::new()),
        }
    }

    fn app(state: AppState) -> Router {
        Router::new()
            .route("/v1/chat", get(metrics))
            .route("/v1/chat/completions", post(completions))
            .with_state(state)
    }

    // ── Metrics endpoint tests ──────────────────────────────────────────────

    #[tokio::test]
    async fn test_metrics_empty_state() {
        let state = test_state(vec![]);
        let app = app(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/chat")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_metrics_with_endpoints() {
        let state = test_state(vec!["http://ep1:8000", "http://ep2:8000"]);
        state.scores.insert("http://ep1:8000".into(), 3.0);
        state.top.insert("top".into(), "http://ep1:8000".into());
        let app = app(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/chat")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
        assert_eq!(json.len(), 2);
    }

    #[tokio::test]
    async fn test_metrics_status_classification() {
        let state = test_state(vec![
            "http://ok:8000",
            "http://bad:8000",
            "http://pending:8000",
        ]);
        state.scores.insert("http://ok:8000".into(), 1.0);
        state.scores.insert("http://bad:8000".into(), f64::MAX);
        // pending has no score entry
        let app = app(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/chat")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();

        let ok_ep = json
            .iter()
            .find(|e| e["endpoint"] == "http://ok:8000")
            .unwrap();
        let bad_ep = json
            .iter()
            .find(|e| e["endpoint"] == "http://bad:8000")
            .unwrap();
        let pending_ep = json
            .iter()
            .find(|e| e["endpoint"] == "http://pending:8000")
            .unwrap();

        assert_eq!(ok_ep["status"], "ok");
        assert_eq!(bad_ep["status"], "unreachable");
        assert_eq!(pending_ep["status"], "pending");
    }

    // ── Completions routing tests ───────────────────────────────────────────

    #[tokio::test]
    async fn test_completions_all_unreachable() {
        let state = test_state(vec!["http://nonexistent:9999"]);
        state
            .scores
            .insert("http://nonexistent:9999".into(), f64::MAX);
        let app = app(state);

        let payload = json!({
            "model": "test-model",
            "messages": [{"role": "user", "content": "hi"}],
            "max_tokens": 10,
            "temperature": 0.7,
            "top_p": 0.9
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_string(&payload).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    }

    #[tokio::test]
    async fn test_completions_fallback_chain() {
        // Start a mock server that returns 500 on first endpoint
        let mut server1 = mockito::Server::new_async().await;
        let _m1 = server1
            .mock("POST", "/v1/chat/completions")
            .with_status(500)
            .create();

        let mut server2 = mockito::Server::new_async().await;
        let _m2 = server2.mock("POST", "/v1/chat/completions")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"choices":[{"finish_reason":"stop","index":0,"logprobs":null,"message":{"content":"Hello","refusal":null,"role":"assistant","tool_calls":null,"function_call":null,"reasoning":null}}],"created":123,"id":"test","model":"test","object":"chat.completion","service_tier":null,"system_fingerprint":null,"usage":{"completion_tokens":1,"prompt_tokens":1,"total_tokens":2}}"#)
            .create();

        let state = AppState {
            client: Arc::new(Client::new()),
            endpoints: vec![server1.url(), server2.url()],
            scores: Arc::new(DashMap::new()),
            top: Arc::new(DashMap::new()),
        };
        state.scores.insert(server1.url(), 1.0);
        state.scores.insert(server2.url(), 2.0);
        let app = app(state);

        let payload = json!({
            "model": "test-model",
            "messages": [{"role": "user", "content": "hi"}],
            "max_tokens": 10,
            "temperature": 0.7,
            "top_p": 0.9
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_string(&payload).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_completions_non_streaming_success() {
        let mut server = mockito::Server::new_async().await;
        let _m = server.mock("POST", "/v1/chat/completions")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"choices":[{"finish_reason":"stop","index":0,"logprobs":null,"message":{"content":"Hi there","refusal":null,"role":"assistant","tool_calls":null,"function_call":null,"reasoning":null}}],"created":123,"id":"abc","model":"test","object":"chat.completion","service_tier":null,"system_fingerprint":null,"usage":{"completion_tokens":2,"prompt_tokens":3,"total_tokens":5}}"#)
            .create();

        let state = AppState {
            client: Arc::new(Client::new()),
            endpoints: vec![server.url()],
            scores: Arc::new(DashMap::new()),
            top: Arc::new(DashMap::new()),
        };
        let app = app(state);

        let payload = json!({
            "model": "test",
            "messages": [{"role": "user", "content": "hi"}],
            "max_tokens": 10,
            "temperature": 0.7,
            "top_p": 0.9
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_string(&payload).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["choices"][0]["message"]["content"], "Hi there");
    }

    #[tokio::test]
    async fn test_completions_streaming_response() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("POST", "/v1/chat/completions")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body("data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\ndata: [DONE]\n\n")
            .create();

        let state = AppState {
            client: Arc::new(Client::new()),
            endpoints: vec![server.url()],
            scores: Arc::new(DashMap::new()),
            top: Arc::new(DashMap::new()),
        };
        let app = app(state);

        let payload = json!({
            "model": "test",
            "messages": [{"role": "user", "content": "hi"}],
            "max_tokens": 10,
            "temperature": 0.7,
            "top_p": 0.9,
            "stream": true
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_string(&payload).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let headers = response.headers();
        assert_eq!(
            headers.get(header::CONTENT_TYPE).unwrap(),
            "text/event-stream; charset=utf-8"
        );
        assert_eq!(headers.get(header::CACHE_CONTROL).unwrap(), "no-cache");
    }
}

#[cfg(test)]
mod stress {
    use axum::http::{Request, StatusCode, header};
    use axum::{Router, body::Body, routing::post};
    use dashmap::DashMap;
    use reqwest::Client;
    use serde_json::json;
    use std::sync::Arc;
    use std::time::Instant;
    use tower::ServiceExt;

    use crate::func::{completions, ordered_endpoints};
    use crate::reqs::AppState;

    fn stress_state(endpoints: Vec<&str>) -> AppState {
        AppState {
            client: Arc::new(
                Client::builder()
                    .timeout(std::time::Duration::from_secs(2))
                    .build()
                    .unwrap(),
            ),
            endpoints: endpoints.iter().map(|s| s.to_string()).collect(),
            scores: Arc::new(DashMap::new()),
            top: Arc::new(DashMap::new()),
        }
    }

    fn app(state: AppState) -> Router {
        Router::new()
            .route("/v1/chat/completions", post(completions))
            .with_state(state)
    }

    #[tokio::test]
    async fn test_50_endpoints_ordering() {
        let endpoints: Vec<String> = (0..50).map(|i| format!("http://ep-{}:8000", i)).collect();
        let state = stress_state(endpoints.iter().map(|s| s.as_str()).collect());

        // Assign varied scores
        for (i, ep) in endpoints.iter().enumerate() {
            state.scores.insert(ep.clone(), (i % 10) as f64);
        }

        let start = Instant::now();
        let ordered = ordered_endpoints(&state);
        let elapsed = start.elapsed();

        assert_eq!(ordered.len(), 50);
        // Lowest score (0) should be first
        assert!(ordered[0].ends_with(":8000"));
        println!("50-endpoint ordering took: {:?}", elapsed);
    }

    #[tokio::test]
    async fn test_50_endpoints_all_unreachable() {
        let endpoints: Vec<String> = (0..50)
            .map(|i| format!("http://nonexistent-{}:9999", i))
            .collect();
        let state = stress_state(endpoints.iter().map(|s| s.as_str()).collect());

        for ep in &endpoints {
            state.scores.insert(ep.clone(), f64::MAX);
        }

        let app = app(state);
        let payload = json!({
            "model": "test",
            "messages": [{"role": "user", "content": "stress test"}],
            "max_tokens": 10,
            "temperature": 0.7,
            "top_p": 0.9
        });

        let start = Instant::now();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_string(&payload).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let elapsed = start.elapsed();

        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        println!("50 unreachable endpoints fallback took: {:?}", elapsed);
    }

    #[tokio::test]
    async fn test_concurrent_requests_50_endpoints() {
        let endpoints: Vec<String> = (0..50)
            .map(|i| format!("http://nonexistent-{}:9999", i))
            .collect();
        let state = stress_state(endpoints.iter().map(|s| s.as_str()).collect());

        for ep in &endpoints {
            state.scores.insert(ep.clone(), f64::MAX);
        }

        let app = app(state);
        let payload = json!({
            "model": "test",
            "messages": [{"role": "user", "content": "concurrent"}],
            "max_tokens": 10,
            "temperature": 0.7,
            "top_p": 0.9
        });
        let payload_str = serde_json::to_string(&payload).unwrap();

        // Fire 100 concurrent requests
        let num_requests = 100;
        let mut handles = Vec::with_capacity(num_requests);
        let start = Instant::now();

        for _ in 0..num_requests {
            let app = app.clone();
            let payload_str = payload_str.clone();
            handles.push(tokio::spawn(async move {
                let response = tower::ServiceExt::oneshot(
                    app,
                    Request::builder()
                        .method("POST")
                        .uri("/v1/chat/completions")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(payload_str))
                        .unwrap(),
                )
                .await
                .unwrap();
                response.status()
            }));
        }

        let results: Vec<StatusCode> = futures_util::future::join_all(handles)
            .await
            .into_iter()
            .map(|h: Result<StatusCode, _>| h.unwrap())
            .collect();

        let elapsed = start.elapsed();

        // All should return BAD_GATEWAY
        assert!(results.iter().all(|s| *s == StatusCode::BAD_GATEWAY));
        println!(
            "{} concurrent requests (50 endpoints each) took: {:?}",
            num_requests, elapsed
        );
        println!(
            "Throughput: {:.2} req/s",
            num_requests as f64 / elapsed.as_secs_f64()
        );
    }

    #[tokio::test]
    async fn test_metrics_scraper_50_endpoints() {
        // Create 50 mock servers responding to /metrics
        let mut mock_urls = Vec::new();
        let mut mocks = Vec::new();

        for i in 0..50 {
            let mut server = mockito::Server::new_async().await;
            let score = (i % 10) as f64;
            let _m = server
                .mock("GET", "/metrics")
                .with_status(200)
                .with_body(&format!(
                    "vllm:num_requests_running {}\nvllm:num_requests_waiting {}\n",
                    score / 2.0,
                    score / 4.0
                ))
                .create();
            mock_urls.push(server.url());
            mocks.push(server);
        }

        // Keep mocks alive
        let _mocks = mocks;

        let state = AppState {
            client: Arc::new(Client::new()),
            endpoints: mock_urls.clone(),
            scores: Arc::new(DashMap::new()),
            top: Arc::new(DashMap::new()),
        };

        // Spawn scraper for one tick cycle
        let state_clone = state.clone();
        let scraper = tokio::spawn(async move {
            // Manually run one scrape cycle instead of looping forever
            let mut handles = Vec::with_capacity(state_clone.endpoints.len());
            for endpoint in &state_clone.endpoints {
                let client = state_clone.client.clone();
                let url = endpoint.clone();
                handles.push(tokio::spawn(async move {
                    let url_metrics = format!("{}/metrics", url);
                    let score = match tokio::time::timeout(
                        std::time::Duration::from_secs(5),
                        client.get(&url_metrics).send(),
                    )
                    .await
                    {
                        Ok(Ok(r)) if r.status().is_success() => r
                            .text()
                            .await
                            .map(|t| crate::func::parse_load_score(&t))
                            .unwrap_or(f64::MAX),
                        _ => f64::MAX,
                    };
                    (url, score)
                }));
            }

            for handle in handles {
                if let Ok((url, score)) = handle.await {
                    state_clone.scores.insert(url, score);
                }
            }
        });

        scraper.await.unwrap();

        // Verify all 50 endpoints have scores
        assert_eq!(state.scores.len(), 50);

        // Verify scores are reasonable (not all MAX)
        let valid_scores = state
            .scores
            .iter()
            .filter(|e| *e.value() < f64::MAX)
            .count();
        assert!(valid_scores > 0);

        // Test top election
        let best = state
            .endpoints
            .iter()
            .filter_map(|u| state.scores.get(u).map(|s| (u.clone(), *s)))
            .filter(|(_, s)| *s < f64::MAX)
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

        assert!(best.is_some());
        let (best_url, best_score) = best.unwrap();
        state.top.insert("top".into(), best_url.clone());

        // Verify ordering puts the best endpoint first
        let ordered = ordered_endpoints(&state);
        assert_eq!(ordered[0], best_url);
        println!(
            "50-endpoint scraper test passed, best score: {:.2}",
            best_score
        );
    }

    #[tokio::test]
    async fn test_high_concurrency_ordering() {
        let endpoints: Vec<String> = (0..50).map(|i| format!("http://ep-{}:8000", i)).collect();
        let state = stress_state(endpoints.iter().map(|s| s.as_str()).collect());

        for (i, ep) in endpoints.iter().enumerate() {
            state.scores.insert(ep.clone(), (i % 10) as f64);
        }

        // 1000 concurrent ordering calls
        let num_calls = 1000;
        let mut handles = Vec::with_capacity(num_calls);
        let start = Instant::now();

        for _ in 0..num_calls {
            let state = state.clone();
            handles.push(tokio::spawn(async move { ordered_endpoints(&state) }));
        }

        let results: Vec<Vec<String>> = futures_util::future::join_all(handles)
            .await
            .into_iter()
            .map(|h| h.unwrap())
            .collect();

        let elapsed = start.elapsed();

        // All orderings should be consistent
        assert!(results.iter().all(|r| r.len() == 50));
        println!(
            "{} concurrent ordering calls took: {:?}",
            num_calls, elapsed
        );
        println!(
            "Ordering throughput: {:.2} ops/s",
            num_calls as f64 / elapsed.as_secs_f64()
        );
    }
}
