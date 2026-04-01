# Kairos — Engineering Mindset Plan

> This is not a coding tutorial. Each phase teaches you how to think about
> a problem before you solve it. The code is yours to figure out.
> The thinking is what we're building here.

---

## Before Any Code — The Mental Models You Need

### How Pingora thinks differently from Axum

In Axum you think in routes:
"When a request comes to `/v1/chat`, call this handler."

In Pingora you think in a pipeline:
"Every request passes through these stages in order. I decide what happens at each stage."

The stages that matter for Kairos:

```
Client sends request
       ↓
request_filter()   ← you can read, modify, or reject the request here
       ↓
upstream_peer()    ← you decide which engine gets this request — THIS IS YOUR ROUTING DECISION
       ↓
[Pingora connects to engine, forwards request, streams response back]
       ↓
response_filter()  ← you can read the response headers here (TTFT lives here)
       ↓
logging()          ← request is done — update your state, record metrics, learn from outcome
```

Your routing brain lives entirely in `upstream_peer()`.
Your learning loop lives entirely in `logging()`.
Everything else is read-only observation.

### How streaming actually works

When an inference engine generates tokens, it doesn't wait until the full response is ready.
It sends each token as it's generated, using a format called SSE:

```
data: {"token": "The"}
data: {"token": " quick"}
data: {"token": " brown"}
data: [DONE]
```

Your router sits between client and engine. It must not buffer this —
it must pass each chunk through the moment it arrives.
Pingora does this for you automatically. You don't write streaming code.
But you need to understand what's flowing through so you know what
`response_filter()` can and cannot see (it sees headers, not body chunks).

### The prefix store in plain English

When request A arrives with prompt "You are a helpful assistant. What is the capital of France?",
your router hashes the first 128 tokens of that prompt and notes "engine-2 handled this."

When request B arrives 10 seconds later with the same system prompt prefix,
your router asks: "has anyone seen this hash before?" — yes, engine-2.
Send it there. Engine-2 already has the KV cache for that prefix.
It skips prefill. Response is faster.

This is the entire value proposition. Every other feature is infrastructure for this insight.

---

## Phase 0 — Read Before You Build (2–3 days)

**Goal:** Understand the tools before using them. No code.

### What to read and why

**Pingora quick start** — `github.com/cloudflare/pingora/blob/main/docs/quick_start.md`
Read the whole thing. Build the load balancer example exactly as written.
You are not building Kairos yet. You are learning how Pingora thinks.
Questions to answer when you're done:
- What is `type CTX`? Why does every request get its own?
- What does `upstream_peer()` return, and what does Pingora do with it?
- Where would you put code that runs after the response comes back?

**Pingora filter/modify guide** — `github.com/cloudflare/pingora/blob/main/docs/user_guide/modify_filter.md`
This shows all the callback hooks. Study the `logging()` example with Prometheus.
That pattern is exactly what you'll use.

**SSE format** — search "Server-Sent Events MDN". Read it. 5 minutes.
You need to know what the engine is actually sending so you understand
what "streaming response" means at the wire level.

**tikv/rust-prometheus README** — `github.com/tikv/rust-prometheus`
Just the README. Understand Counter vs Gauge vs Histogram.
Know when to use each before you define any metrics.

### Questions to answer before Phase 1

Write these answers in a `DESIGN.md` in your repo. Forces clarity.

1. What is the difference between `request_filter()` and `upstream_peer()`?
   When would you reject a request vs. route it?

2. What information do you have available in `upstream_peer()` that you
   don't have in `logging()`? And vice versa?

3. A Counter goes up. A Gauge goes up and down. A Histogram records distributions.
   Which one is right for: request count? engine queue depth? request duration?

4. If the engine goes down mid-request, which Pingora callback fires?
   How would you detect this?

---

## Phase 1 — Working Proxy (1 week)

**Goal:** Kairos accepts requests and forwards them. Nothing smart yet. Just working.

### The engineering thinking for this phase

Before writing a single line, draw the data flow on paper:

```
Client → Kairos (Pingora, port 8080) → Engine
                    ↓
            Axum admin (port 9090)
```

Ask yourself: what does `KairosProxy` need to know to forward a request?
- The list of engines (addresses)
- Which one to pick (for now: round-robin)

That's it. Everything else is ceremony. Don't add anything you don't need yet.

### How to think about each piece

**`metrics.rs` first — before proxy logic**
Define all metrics upfront. This is not premature optimization —
it's forcing you to name and understand every signal your system produces
before you build the system that produces them.
If you can't name a metric, you don't understand what you're measuring.

**`RequestContext`**
This is your per-request scratchpad. Pingora creates one for each request
and passes it through every callback. Ask: what does `logging()` need to know
that only `upstream_peer()` can see? Those things go in `RequestContext`.
Start minimal. Add fields only when a later phase needs them.

**`upstream_peer()` — the only method Pingora requires**
Everything else is optional. Start with just this.
It takes a `Session` (the request) and your `CTX`, returns an `HttpPeer` (an engine address).
Round-robin means: take a counter, mod by engine count, return that engine.

**The Axum admin server**
It runs on a different port in a separate `tokio::task`.
It shares state with the proxy via `Arc`.
`/metrics` just calls `prometheus::gather()` and formats it as text.
`/health` returns 200. That's all Phase 1 needs.

### How to know Phase 1 is done

- Send 100 requests, verify they split across engines
- Kill one mock engine, verify Pingora surfaces an error (it will — no circuit breaker yet)
- Hit `/metrics`, see `requests_total` counting up
- Hit `/health`, get 200

---

## Phase 2 — Know Your Fleet (1 week)

**Goal:** Kairos knows the live state of each engine and routes based on it.

### The engineering thinking for this phase

In Phase 1, routing was stateless — you knew nothing about the engines except their addresses.
In Phase 2, routing becomes stateful — you maintain a live snapshot of each engine's health.

The core question: **how does stale data affect routing decisions?**

If your fleet snapshot is 10 seconds old and engine-2 just got a burst of 50 requests,
`LeastLoad` might still think engine-2 is idle and send more traffic there.
This is the tradeoff between polling frequency and system load.
There is no right answer. Pick a value (1s), document why, tune later.

### How to think about each piece

**`FleetMonitor` — a background task, not a request handler**
This is new territory compared to Axum. In Axum everything was request-driven.
Here you have a loop that runs independently of requests:

```
every 1 second:
  for each engine:
    GET /health → parse response → update FleetState
```

It writes to `Arc<RwLock<FleetState>>`.
The proxy reads from the same `Arc<RwLock<FleetState>>` in `upstream_peer()`.

Think about this: the monitor writes, the proxy reads, concurrently.
Why `RwLock` and not `Mutex`? 
Answer: many requests read simultaneously, only one monitor writes.
`RwLock` allows concurrent readers. `Mutex` does not.

**EMA (Exponentially Weighted Moving Average)**
You can't use raw TTFT values — they're too noisy.
A single slow request would make an engine look bad for a long time.
EMA smooths this: `new_ema = 0.1 * latest_sample + 0.9 * current_ema`
The 0.1/0.9 split means recent data matters more than old data,
but old data doesn't disappear instantly.
This is the right mental model for any "current performance" metric.

**`LeastLoad` and `LowestLatency`**
These are not intelligent strategies. They are greedy.
They look at current state and pick the best-looking engine right now.
They have no memory. They don't learn. They're fast and simple.
Understanding their failure modes before building them:
- `LeastLoad` fails when queue_depth is stale
- `LowestLatency` fails when EMA hasn't converged (cold start)
You'll fix these failure modes in Phase 6.

**Strategy selection by env var**
`KAIROS_STRATEGY=least_load` → pick `LeastLoad`
This is how you test strategies in isolation before the bandit takes over.
Don't build a fancy config system. One env var, one match statement.

### How to know Phase 2 is done

- Mock engines return different queue depths — `LeastLoad` consistently picks the right one
- Kill an engine — fleet monitor marks it unhealthy within 2s — no requests go there
- `engine_queue_depth` and `engine_healthy` Prometheus gauges reflect live state

---

## Phase 3 — Prefix Awareness (1 week)

### Part A — Understanding the prefix store

**Before building anything, understand the problem deeply.**

An inference engine keeps a KV cache — a memory of the attention computation
for tokens it has already processed. If request B shares a prefix with request A,
and both go to the same engine, request B's prefill is partially free.

Your router cannot see inside the engine's KV cache.
You don't have an API for "does engine-2 have this prefix cached?"
You have to infer it from history: "engine-2 handled a request with this prefix 3 seconds ago,
so it probably still has it cached."

This is an approximation. It's the right approximation.
The alternative — trying to read the engine's cache state — is what MAXimus couldn't do.
You are building the metadata layer that makes cache-aware routing possible
without the engine exposing it.

### Part B — How to think about tokenization

You tokenize the prompt to get a sequence of integer IDs.
`"You are a helpful assistant"` → `[1, 341, 389, 257, 7613, 8796]`

You hash prefixes of this sequence at multiple depths:
- First 128 token IDs → one hash (coarse match)
- First 256 token IDs → one hash (medium match)  
- First 512 token IDs → one hash (fine match)

Why multiple depths? Because you want to match even if only the system prompt is shared,
not just when the entire conversation prefix matches.
A coarse match (128 tokens) is worth something. A fine match (512 tokens) is worth more.
Your `PrefixLocality` strategy will score engines by match depth.

### Part C — How to think about the WAL

The prefix store lives in memory. If Kairos restarts, it's gone.
Without it, the first few minutes after restart have no prefix-aware routing.
This is acceptable for a cache (worst case: cache miss). But you can do better.

A WAL (Write-Ahead Log) is just an append-only file.
Every time you update the store, you write the update to the file first.
On startup, you replay the file to rebuild the store.

The rules:
- Write to disk before updating memory (not after — "write-ahead" means exactly this)
- Use CRC32 to detect partial writes (crash mid-write leaves corrupt entry — skip it)
- Flush every 500ms, not every write — fsync is expensive
- The store is soft-state (cache metadata, not user data) — losing the last 500ms is fine

### Part D — The timing problem

TTFT arrives in `response_filter()`.
Store update happens in `logging()`.
These are different callbacks.

You bridge them via `RequestContext`:
- `response_filter()` sets `ctx.ttft_ms = Some(elapsed)`
- `logging()` reads `ctx.ttft_ms` and updates the store

`RequestContext` is the carrier. This is why you define it in Phase 1
even though you don't fully use it until Phase 3.

### How to know Phase 3 is done

- Same prompt twice → second request goes to same engine
- Recheck after restart → same engine on third request (WAL replay worked)
- `prefix_hit_total` counter increases on second request

---

## Phase 4 — The Bandit (1 week)

### Part A — Understand the problem before the algorithm

You have 4 strategies: RoundRobin, LeastLoad, LowestLatency, PrefixLocality.
Each performs differently depending on traffic patterns:
- Repetitive prompts (same system prompt) → PrefixLocality wins
- Bursty diverse traffic → LeastLoad wins  
- Stable load, varying latency → LowestLatency wins

You don't know which pattern you're in. And it changes over time.

The naive solution: pick one strategy and hope.
The bandit solution: try all strategies, measure which performs best right now, do more of that.

### Part B — The exploration/exploitation tradeoff

Here is the core tension:

**Exploitation:** use the strategy that has worked best so far.
**Exploration:** try other strategies in case they'd work even better.

If you only exploit, you might be stuck on a mediocre strategy because you never tried the others.
If you only explore, you waste requests on bad strategies even when you know a good one.

UCB (Upper Confidence Bound) resolves this with one elegant idea:
**be optimistic about things you haven't tried much.**

Each strategy has a score:
```
score = mean_reward + C * sqrt(ln(total_pulls) / pulls_for_this_arm)
```

The first term is exploitation — how good has this strategy been?
The second term is exploration — how uncertain are you about this strategy?

Strategies you've tried less have higher uncertainty → higher score → get tried more.
As you try them more, uncertainty drops → score reflects true performance.
`C` controls the balance. Start with 1.0.

**Implement this yourself.** The formula is 5 lines. The insight is worth more than the code.

### Part C — Sliding window, not running mean

Traffic patterns change. An hour ago, PrefixLocality was dominant.
Right now, the workload shifted to diverse prompts and LeastLoad is better.

If you track all rewards ever observed (running mean), old data dilutes new data.
You'd be slow to adapt.

Use a sliding window: keep only the last 200 rewards per strategy.
Old data falls off automatically. You track current performance, not historical average.

`VecDeque<f64>` with a capacity cap of 200. Push back, pop front when full.

### Part D — The reward signal

Reward = inverse of TTFT. Lower latency = higher reward.
`reward = 1000.0 / ttft_ms`

This is in `logging()`. It flows to the bandit via `tokio::sync::mpsc`.
The proxy sends reward, the bandit receives and updates, independently.
They don't block each other.

### Part E — Cold start

On startup, no arm has been pulled. `rewards` is empty.
UCB formula breaks (division by zero).
Rule: if `rewards.is_empty()`, return `f64::MAX` — always try unplayed arms first.
This gives you a brief exploration phase at startup, then UCB takes over.

### How to know Phase 4 is done

- Simulate bursty diverse traffic → bandit converges to LeastLoad within ~50 requests
- Simulate repetitive prompts → bandit converges to PrefixLocality within ~50 requests  
- `bandit_arm_reward` Prometheus gauge shows per-strategy reward trends
- Convergence visible in metrics without looking at logs

---

## Phase 5 — LinUCB / RL (when bandit plateaus)

**Don't start this phase until you have evidence UCB is underperforming.**

### The problem UCB doesn't solve

UCB picks the best strategy globally. But "best" depends on context.
At 9am with repetitive agent workloads, PrefixLocality is best.
At 3pm with diverse user traffic, LeastLoad is best.
UCB doesn't know about time of day. It picks one winner.

LinUCB fixes this by conditioning the strategy selection on a context vector:
```
context = [prefix_hit_rate, fleet_load_variance, requests_per_sec, hour_of_day]
```

Each strategy now has a weight vector. Score = weights · context + exploration bonus.
The agent learns which context features predict which strategy's performance.

### Why Candle/Burn enters here

LinUCB requires matrix operations: `A⁻¹b` where A is 4×4.
This is where a tensor library earns its place — clean matrix math, GPU-optional.

DQN (if you go further) requires a small neural network.
Candle and Burn are both options. Candle has a more active ecosystem.
Pick one and stay with it.

### The discipline for this phase

Log the context vector from Phase 4 onward (even before you use it).
By the time you build LinUCB, you'll have real data to validate against.
Never build an ML system without first looking at the data it will train on.

---

## Phase 6 — Hardening

**Goal:** The system survives reality.

### Circuit breaker — the most important thing in this phase

A circuit breaker is a pattern, not a library.
Concept: if engine-2 fails 5 requests in a row, stop sending traffic there for 30 seconds.
After 30 seconds, send one test request. If it succeeds, re-open. If not, wait again.

States: Closed (normal) → Open (failing, no traffic) → Half-Open (testing)

This is essential because without it, a failing engine receives traffic, fails, and
your Prometheus metrics show degraded performance for all users until you notice and intervene.
With it, the router heals itself.

### Retry logic

On upstream error, pick the next-best engine and retry once.
Don't retry indefinitely — you'll amplify load on a struggling fleet.
One retry, then fail with a 502.

### Config file

`kairos.toml` with: engine addresses, ports, bandit `C` constant, WAL path, flush interval.
Read at startup. No hot-reload yet — that's operational complexity you don't need.

### What "production-ready" actually means

It doesn't mean perfect. It means:
- It fails gracefully (circuit breaker, retry)
- It tells you what's wrong (metrics, logs)
- It recovers from restarts (WAL)
- It shuts down cleanly (drain in-flight requests)

---

## The One Thread Running Through Every Phase

Every phase asks the same question in a different form:

**What do I know, when do I know it, and what decision does it enable?**

Phase 1: I know the engine addresses. I decide by position.
Phase 2: I know engine health right now. I decide by current load.
Phase 3: I know prefix history. I decide by cache locality.
Phase 4: I know which strategy has worked recently. I decide by learned performance.
Phase 5: I know what kind of traffic this is. I decide by context.

Each phase adds one new signal. The routing decision gets smarter by one dimension each time.
That's the architecture. Simple, additive, testable at each step.
