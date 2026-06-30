use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::{
    collections::HashMap,
    net::IpAddr,
    sync::{Arc, Mutex},
    time::Instant,
};

use crate::node::NodeMetrics;

// ── Token bucket parameters per route class ───────────────────────────────────

/// General endpoints: 100 req burst, refill 100/min.
const STANDARD_CAPACITY: f64 = 100.0;
const STANDARD_REFILL: f64 = 100.0 / 60.0;

/// Transaction submission: 20 req burst, refill 20/min.
const SUBMIT_CAPACITY: f64 = 20.0;
const SUBMIT_REFILL: f64 = 20.0 / 60.0;

/// Faucet: 5 req burst, refill 5/hour.
const FAUCET_CAPACITY: f64 = 5.0;
const FAUCET_REFILL: f64 = 5.0 / 3600.0;

// ── Token bucket ──────────────────────────────────────────────────────────────

struct TokenBucket {
    tokens: f64,
    last_refill: Instant,
    capacity: f64,
    refill_rate: f64, // tokens per second
}

impl TokenBucket {
    fn new(capacity: f64, refill_rate: f64) -> Self {
        Self { tokens: capacity, last_refill: Instant::now(), capacity, refill_rate }
    }

    /// Refills based on elapsed time then tries to consume one token.
    /// Returns `true` if the request is allowed.
    fn try_consume(&mut self) -> bool {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.refill_rate).min(self.capacity);
        self.last_refill = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

// ── Route classification ──────────────────────────────────────────────────────

#[derive(Clone, Copy)]
enum RouteClass {
    /// No limit — health and metrics must never be throttled.
    Exempt,
    /// Default limit: 100 burst / 100 per minute.
    Standard,
    /// Stricter: 20 burst / 20 per minute.
    Submit,
    /// Very strict: 2 burst / 2 per hour.
    Faucet,
}

fn classify(path: &str) -> RouteClass {
    match path {
        "/health" | "/metrics" | "/ws" | "/events" => RouteClass::Exempt,
        "/tx/submit" => RouteClass::Submit,
        p if p.starts_with("/faucet") => RouteClass::Faucet,
        _ => RouteClass::Standard,
    }
}

// ── Per-IP bucket set ─────────────────────────────────────────────────────────

struct IpBuckets {
    standard: TokenBucket,
    submit: TokenBucket,
    faucet: TokenBucket,
}

impl IpBuckets {
    fn new() -> Self {
        Self {
            standard: TokenBucket::new(STANDARD_CAPACITY, STANDARD_REFILL),
            submit: TokenBucket::new(SUBMIT_CAPACITY, SUBMIT_REFILL),
            faucet: TokenBucket::new(FAUCET_CAPACITY, FAUCET_REFILL),
        }
    }
}

// ── RateLimiter ───────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct RateLimiter {
    buckets: Arc<Mutex<HashMap<IpAddr, IpBuckets>>>,
    metrics: NodeMetrics,
}

impl RateLimiter {
    pub fn new(metrics: NodeMetrics) -> Self {
        Self { buckets: Arc::new(Mutex::new(HashMap::new())), metrics }
    }
}

// ── Axum middleware ───────────────────────────────────────────────────────────

pub async fn rate_limit(
    ConnectInfo(addr): ConnectInfo<std::net::SocketAddr>,
    axum::extract::State(limiter): axum::extract::State<RateLimiter>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let path = request.uri().path().to_owned();
    let class = classify(&path);

    if matches!(class, RouteClass::Exempt) {
        return next.run(request).await;
    }

    let ip = addr.ip();
    let allowed = {
        let mut map = limiter.buckets.lock().unwrap();
        let buckets = map.entry(ip).or_insert_with(IpBuckets::new);
        match class {
            RouteClass::Submit => buckets.submit.try_consume(),
            RouteClass::Faucet => buckets.faucet.try_consume(),
            RouteClass::Standard | RouteClass::Exempt => buckets.standard.try_consume(),
        }
    };

    if !allowed {
        limiter
            .metrics
            .ratelimit_hit
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let msg = match class {
            RouteClass::Submit => "Rate limit exceeded — max 20 requests/minute for /tx/submit",
            RouteClass::Faucet => "Rate limit exceeded — max 2 requests/hour for /faucet",
            _ => "Rate limit exceeded — max 100 requests/minute",
        };
        return (StatusCode::TOO_MANY_REQUESTS, msg).into_response();
    }

    next.run(request).await
}
