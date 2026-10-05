//! CORS for the public API, so web pages (the showcase site, explorers, merchant
//! tills) can read the chain and submit signed transactions from a browser.
//!
//! Any origin is allowed, without credentials: the public API is unauthenticated
//! and every transaction carries its own signature, so a foreign page gains nothing
//! it couldn't do with curl. Operator routes (`/admin*`, `/snapshot`) get no CORS
//! headers, so browsers keep blocking cross-site calls to them.

use axum::{
    extract::Request,
    http::{header, HeaderValue, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};

fn is_public(path: &str) -> bool {
    !(path == "/admin" || path.starts_with("/admin/") || path == "/snapshot")
}

fn add_headers(res: &mut Response) {
    let h = res.headers_mut();
    h.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    h.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, POST, OPTIONS"),
    );
    h.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("content-type"),
    );
    h.insert(
        header::ACCESS_CONTROL_MAX_AGE,
        HeaderValue::from_static("86400"),
    );
}

pub async fn cors(req: Request, next: Next) -> Response {
    if !is_public(req.uri().path()) {
        return next.run(req).await;
    }
    // Preflight: answered here, before the rate limiter and the router.
    if req.method() == Method::OPTIONS {
        let mut res = StatusCode::NO_CONTENT.into_response();
        add_headers(&mut res);
        return res;
    }
    let mut res = next.run(req).await;
    add_headers(&mut res);
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, middleware, routing::get, Router};
    use tower::ServiceExt;

    fn app() -> Router {
        Router::new()
            .route("/health", get(|| async { "ok" }))
            .route("/admin/compact", get(|| async { "ok" }))
            .layer(middleware::from_fn(cors))
    }

    async fn call(method: Method, path: &str) -> Response {
        let req = Request::builder()
            .method(method)
            .uri(path)
            .header("origin", "https://example.org");
        app()
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn public_routes_allow_any_origin() {
        let res = call(Method::GET, "/health").await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*");
        assert!(res
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_CREDENTIALS)
            .is_none());
    }

    #[tokio::test]
    async fn preflight_is_answered() {
        let res = call(Method::OPTIONS, "/tx/submit").await;
        assert_eq!(res.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            res.headers()[header::ACCESS_CONTROL_ALLOW_HEADERS],
            "content-type"
        );
    }

    #[tokio::test]
    async fn admin_routes_stay_same_origin() {
        let res = call(Method::GET, "/admin/compact").await;
        assert!(res
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .is_none());
        let res = call(Method::OPTIONS, "/admin/compact").await;
        assert!(res
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .is_none());
    }
}
