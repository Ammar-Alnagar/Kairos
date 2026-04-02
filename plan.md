# Kairos — Engineering Mindset Plan (Updated)

> This is not a coding tutorial. Each phase teaches you how to think about
> a problem before you solve it. The code is yours to figure out.

## Before Any Code — The Mental Models You Need
### How Pingora thinks differently from Axum
(Same as original)

### How streaming actually works
(Same as original)

### The prefix store in plain English
(Same as original — prefix awareness remains)

## Phase 0 — Read Before You Build (2–3 days)
**Goal:** Understand the tools before using them. No code.
(Same reading list as original)

**Updated questions to answer in DESIGN.md:**
1. What is the difference between `request_filter()` and `upstream_peer()`?
2. What information do you have available in `upstream_peer()` vs `logging()`?
3. Which Prometheus metric type for: request count? engine queue depth? request duration?
4. How will you scrape engine `/metrics` endpoints for queue depth and health?

## Phase 1 — Working Proxy (1 week)
**Goal:** Kairos accepts requests and forwards them. Nothing smart yet.
(Same as original: round-robin, metrics, Axum admin, etc.)

## Phase 2 — Know Your Fleet (1 week)
**Goal:** Kairos knows the live state of each engine and routes based on it.

**Key change:**
- Remove active health polling loop.
- Instead, background scraper task every 1s:
  - For each engine: `GET /metrics`
  - Parse Prometheus metrics for `queue_depth`, `healthy` (or infer from response), TTFT/EMA.
- Update `FleetState` from scraped data.
- `LeastLoad` and `LowestLatency` now use scraped values.

**How to know Phase 2 is done:**
- Engines expose different queue depths in `/metrics` → `LeastLoad` picks correctly.
- Engine down → scraper marks unhealthy → no traffic sent.
- Metrics reflect scraped state.

## Phase 3 — Prefix Awareness (1 week)
**Goal:** Cache-aware routing via prefix store + WAL.
(Same as original — tokenizer dropped only for RL, prefix logic unchanged.
Use simple token hashing on prompt text or first N characters if needed.)

**How to know Phase 3 is done:**
(Same as original)

## Phase 4 — The Bandit (1 week)
**Goal:** Dynamically choose between strategies (RoundRobin, LeastLoad, LowestLatency, PrefixLocality) using UCB.

**Key change:**
- Reward signal now comes from scraped engine `/metrics` (e.g. inverse of observed TTFT or success rate) instead of per-request RL.
- Still update bandit in `logging()` with observed outcome + scraped data.

**How to know Phase 4 is done:**
- Simulate traffic patterns → bandit converges to best strategy.
- `bandit_arm_reward` gauge shows trends.

## Phase 5 — Hardening
**Goal:** Survive reality.
- Circuit breaker, retry logic, config file.
- Scraper must handle engine downtime gracefully.
(Same as original)

## The One Thread Running Through Every Phase
**What do I know, when do I know it, and what decision does it enable?**
- Phase 2 now knows fleet state via scraped `/metrics`.
- All later phases build on scraped signals instead of active polling or RL.

**Dropped:** Tokenization for RL context, full LinUCB/DQN.
**Kept:** Prefix store, bandit over strategies, scraping for fleet state.
