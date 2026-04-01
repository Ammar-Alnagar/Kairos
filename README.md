# Kairos

**Adaptive inference router with a learning routing plane.**

Kairos sits in front of a fleet of LLM inference engines. It routes each request to the optimal engine — not by round-robin or random load balancing, but by learning which routing strategy performs best for which traffic patterns, in real time.

The name comes from the Greek concept of *kairos* — the right moment, the optimal timing. That's the problem: given this request, right now, which engine is the right one?

---

## The Problem

Inference routers today are dumb. They do round-robin, least-connections, or at best hash the prompt to a fixed engine. None of them:

- Know which engine has a warm KV cache for this prompt's prefix
- Learn that prefix-locality routing outperforms load-balancing at 9am but not at 3am
- Adapt when a new engine joins the fleet or an old one degrades
- Treat routing as a decision problem with measurable, learnable reward

The result: redundant prefill computation, wasted GPU cycles, and latency that could have been avoided by sending the request to the engine that already cached this prompt.

---

## What Kairos Does

**1. Prefix-aware routing**
Maintains a metadata store of which engine holds KV cache for which prompt prefixes. On each request, finds the longest matching cached prefix and scores engines accordingly.

**2. Multiple routing strategies**
Kairos implements a portfolio of routing strategies:
- `PrefixLocality` — route to the engine with the longest cached prefix match
- `LeastLoad` — route to the engine with the lowest queue depth
- `LowestLatency` — route to the engine with the best recent TTFT
- `RoundRobin` — baseline, no intelligence
- `Random` — baseline, no intelligence

**3. A learning routing plane**
Kairos treats strategy selection as a multi-armed bandit problem. Each strategy is an arm. The reward signal is request latency (TTFT + TBT). A contextual bandit or lightweight RL agent observes the current state (traffic pattern, time of day, fleet load, prefix hit rate) and selects which strategy to apply — updating its policy based on observed outcomes.

Over time: Kairos learns that prefix-locality routing dominates during repetitive agent workloads, while least-load dominates during bursty diverse traffic. It does this without configuration.

**4. Prefix metadata store**
An internal component — a fast in-memory KV store with a prefix-matching index. Tracks per-engine cache state: which prefixes are hot, eviction pressure, last access time. Updated on every request completion. Persisted via a WAL so it survives restarts.

---

## Architecture

```
┌──────────────────────────────────────────────────────┐
│                        Kairos                        │
│                                                      │
│   ┌────────────┐     ┌──────────────────────────┐   │
│   │  Listener  │────▶│      Routing Brain        │   │
│   │ (TCP/HTTP) │     │                          │   │
│   └────────────┘     │  ┌────────────────────┐  │   │
│                      │  │  Bandit / RL Agent  │  │   │
│                      │  │  selects strategy   │  │   │
│                      │  └────────────────────┘  │   │
│                      │                          │   │
│                      │  ┌────────────────────┐  │   │
│                      │  │  Strategy Portfolio │  │   │
│                      │  │  PrefixLocality     │  │   │
│                      │  │  LeastLoad          │  │   │
│                      │  │  LowestLatency      │  │   │
│                      │  │  RoundRobin         │  │   │
│                      │  └────────────────────┘  │   │
│                      └──────────────────────────┘   │
│                                    │                 │
│   ┌────────────────────────────────▼──────────────┐ │
│   │            Prefix Metadata Store              │ │
│   │   prefix_hash → { engine_id, hit_count,       │ │
│   │                   last_access_ms,             │ │
│   │                   eviction_pressure }         │ │
│   │                    + WAL                      │ │
│   └───────────────────────────────────────────────┘ │
│                                    │                 │
│   ┌────────────────────────────────▼──────────────┐ │
│   │              Fleet Monitor                    │ │
│   │   per-engine: queue_depth, ttft_p50,          │ │
│   │               ttft_p99, gpu_util, is_healthy  │ │
│   └───────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────┘
              │                │               │
         Engine 0          Engine 1        Engine 2
        (SGLang/           (vLLM/           (MAX/
         H100)              H200)            B200)
```

---

## Components

### Listener
Accepts incoming inference requests. Speaks OpenAI-compatible HTTP (`/v1/chat/completions`) so it's a drop-in in front of any OpenAI-compatible engine fleet. Implemented in Rust with Axum.

### Routing Brain
The decision core. On each request:
1. Extracts the prompt prefix hash
2. Queries the prefix metadata store for engine coverage
3. Queries the fleet monitor for current engine state
4. Passes the combined state vector to the bandit/RL agent
5. Agent selects a strategy
6. Strategy returns an engine
7. Request is forwarded

### Bandit / RL Agent
The learning component. Two options under consideration:

**Option A — Contextual Bandit (UCB / Thompson Sampling)**
State: `[prefix_hit_rate, fleet_load_variance, traffic_burstiness, time_bucket]`
Action: which strategy to apply
Reward: inverse TTFT (lower latency = higher reward)
Update: online, after every request completion

**Option B — Lightweight RL (Q-learning or policy gradient)**
Same state/action/reward framing but with a learned value function over sequences of decisions. More powerful, more complex, slower to converge.

Start with Option A. Promote to Option B if bandit performance plateaus.

### Strategy Portfolio
Each strategy implements a single trait:

```rust
trait RoutingStrategy {
    fn select_engine(
        &self,
        request: &Request,
        prefix_meta: &PrefixMetaStore,
        fleet: &FleetState,
    ) -> EngineId;
}
```

Strategies are stateless. All state lives in the metadata store and fleet monitor.

### Prefix Metadata Store
Internal fast-path store. Keys are prefix hashes (`u64`), values are engine metadata structs. Backed by a `HashMap` with a prefix-matching index. WAL for durability. Not exposed externally — internal component only.

### Fleet Monitor
Background task that polls each engine's health and metrics endpoint. Maintains a snapshot of current fleet state used by routing strategies. Updates on a configurable interval (default: 1s).

---

## Tech Stack

| Layer | Technology |
|---|---|
| Language | Rust |
| Async runtime | Tokio |
| HTTP server | Axum |
| HTTP client (engine proxy) | reqwest |
| Serialization | serde + bincode (internal), JSON (HTTP boundary) |
| WAL | Custom, append-only with CRC framing |
| Prefix index | radix_trie or custom, TBD |
| Bandit/RL | Custom implementation — no ML framework needed for a bandit |

---

## Build Phases

**Phase 1 — Dumb router**
Axum server, round-robin strategy, health checks, request forwarding. No learning, no prefix awareness. Just a working router.

**Phase 2 — Fleet monitor + strategy portfolio**
Background fleet state poller. Implement LeastLoad and LowestLatency strategies alongside RoundRobin. Strategy is selected by config, not learned yet.

**Phase 3 — Prefix metadata store**
Prefix hash → engine metadata. Updated on request completion. PrefixLocality strategy added to portfolio.

**Phase 4 — Bandit routing plane**
Contextual bandit wrapping the strategy portfolio. Reward signal wired up. Online learning loop running.

**Phase 5 — Hardening**
WAL persistence, graceful shutdown, connection pooling, observability (Prometheus metrics endpoint), chaos testing.

**Phase 6 — RL upgrade (optional)**
Swap bandit for a lightweight RL agent if bandit performance plateaus on complex traffic patterns.

---

## What This Demonstrates

- Production Rust async systems (Axum, Tokio, connection pooling)
- Intelligent distributed systems design (not just plumbing)
- Online learning applied to infrastructure (rare, high-signal)
- Prefix-aware KV cache routing (directly relevant to LLM inference roles)
- Full systems ownership: protocol design, persistence, observability, learning

---

## Status
`[ ] Phase 1 in progress`
