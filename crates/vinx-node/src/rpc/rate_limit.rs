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

const MAX_RPS: u32 = 100;
const WINDOW_SECS: u64 = 60;

#[derive(Clone)]
pub struct RateLimiter {
    state: Arc<Mutex<HashMap<IpAddr, (u32, Instant)>>>,
}

impl RateLimiter {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

pub async fn rate_limit(
    ConnectInfo(addr): ConnectInfo<std::net::SocketAddr>,
    axum::extract::State(limiter): axum::extract::State<RateLimiter>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let ip = addr.ip();
    let now = Instant::now();
    {
        let mut map = limiter.state.lock().unwrap();
        let entry = map.entry(ip).or_insert((0, now));
        if now.duration_since(entry.1).as_secs() >= WINDOW_SECS {
            *entry = (0, now);
        }
        entry.0 += 1;
        if entry.0 > MAX_RPS {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                "Rate limit exceeded — max 100 requests/minute per IP",
            )
                .into_response();
        }
    }
    next.run(request).await
}
