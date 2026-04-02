# Kairos
**Adaptive inference router with a learning routing plane.**

Kairos sits in front of a fleet of LLM inference engines. It routes each request to the optimal engine by learning which routing strategy performs best for current traffic patterns in real time.

## The Problem
Inference routers today are dumb (round-robin, least-connections, or static hashing). None adapt to KV cache locality or learn from observed performance.

## What Kairos Does
**1. Prefix-aware routing**  
Maintains a metadata store of which engine holds KV cache for prompt prefixes. Routes to longest matching prefix when beneficial.

**2. Multiple routing strategies**
- `PrefixLocality` — longest cached prefix match
- `LeastLoad` — lowest queue depth
- `LowestLatency` — best recent TTFT
- `RoundRobin` — baseline

**3. A learning routing plane**  
Treats strategy selection as a multi-armed bandit (UCB). Observes outcomes and shifts weight toward the best-performing strategy for current conditions.

**4. Prefix metadata store**  
In-memory store with WAL persistence. Updated on request completion.

## Architecture
```
Kairos
├── Listener (Pingora)
├── Routing Brain
│   ├── Strategy Portfolio (PrefixLocality, LeastLoad, LowestLatency, RoundRobin)
│   └── Bandit (UCB) — selects strategy
├── Prefix Metadata Store (+ WAL)
└── Fleet Scraper
    └── Scrapes each engine /metrics every 1s for queue_depth, TTFT, health
```

## Components
### Listener
Pingora-based proxy. Accepts OpenAI-compatible `/v1/chat/completions` requests.

### Routing Brain
On each request:
1. Computes prompt prefix hash
2. Queries prefix store
3. Reads current fleet state from scraper
4. Bandit selects strategy
5. Strategy picks engine

### Bandit
UCB bandit over the four strategies. Reward = inverse TTFT from observed response.

### Strategy Portfolio
Stateless strategies implementing a common trait.

### Prefix Metadata Store
In-memory prefix hash → engine metadata. Persisted via WAL.

### Fleet Scraper
Background task that scrapes each engine’s `/metrics` endpoint every 1s. Updates fleet state with queue depth, health, and latency metrics. (Replaces active polling.)

## Tech Stack
| Layer | Technology |
|-------|------------|
| Language | Rust |
| Async | Tokio |
| Proxy | Pingora |
| Metrics | rust-prometheus |
| WAL | Custom append-only with CRC |

## Build Phases
**Phase 1 — Working Proxy**  
Pingora listener, round-robin, Axum admin/metrics. Basic forwarding.

**Phase 2 — Know Your Fleet**  
Background scraper for engine `/metrics`. LeastLoad + LowestLatency strategies. Fleet state from scraped data.

**Phase 3 — Prefix Awareness**  
Prefix metadata store + WAL. PrefixLocality strategy.

**Phase 4 — The Bandit**  
UCB bandit selecting among strategies based on observed rewards.

**Phase 5 — Hardening**  
Circuit breaker, retry, config, graceful shutdown.

## What This Demonstrates
- Production Rust systems with Pingora
- Intelligent routing via prefix locality and online learning
- Scraping-based fleet observability instead of active polling

## Status
`[ ] Phase 1 in progress`
```

**Updates applied:** Dropped tokenizer/RL/LinUCB, replaced fleet monitor polling with /metrics scraping, simplified accordingly.
