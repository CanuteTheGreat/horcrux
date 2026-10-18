//! The strict per-IP auth rate limit (5/min in production) covers only the
//! credential-submitting POSTs (login, tfa, register). The login page's
//! reads (realm list, CA download) and the OpenID Connect redirect/callback
//! used to share that bucket, so a few login-page views made the SSO
//! callback answer 429 and left the user on a raw JSON error page.
//!
//! Runs the real `horcrux-api` binary with the production-sized auth limit
//! (`AUTH_RATE_LIMIT_MAX_REQUESTS=5`; the shared CI server raises it).

use crate::support::{scratch, Server};
use serde_json::json;

#[tokio::test]
async fn sso_and_login_page_reads_do_not_use_up_the_login_rate_limit() {
    let dir = scratch("auth-rate-limit");
    let mut srv = Server::new(&dir).env("AUTH_RATE_LIMIT_MAX_REQUESTS", "5");
    srv.start();
    srv.wait_healthy().await;
    let c = srv.client();
    let base = srv.base();

    // Far more than 5 login-page loads, CA downloads and OIDC round trips
    // from one IP inside a minute: none of them is rate limited.
    for i in 0..8 {
        for path in [
            "/auth/realms",
            "/tls/ca.pem",
            "/auth/oidc/no-such-realm/login",
            "/auth/oidc/callback?code=not-a-code&state=not-a-state",
        ] {
            let r = c.get(format!("{base}{path}")).send().await.unwrap();
            assert_ne!(
                r.status().as_u16(),
                429,
                "GET {path} rate limited on round {i}: {}",
                r.text().await.unwrap_or_default()
            );
        }
    }

    // ... and after all that, the first real login still works.
    let r = c
        .post(format!("{base}/auth/login"))
        .json(&json!({"username": "admin", "password": crate::support::admin_password()}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 200, "login after page views");

    // Password guessing is still capped: login, tfa and register share the
    // strict bucket (1 used above), so the 5th attempt after it is refused.
    let mut statuses = Vec::new();
    for i in 0..5 {
        let (path, body) = match i % 3 {
            0 => (
                "/auth/login",
                json!({"username": "admin", "password": "wrong"}),
            ),
            1 => ("/auth/tfa", json!({"ticket": "x", "code": "000000"})),
            _ => (
                "/auth/register",
                json!({"username": "rl-x", "password": "rl-x-pass-1"}),
            ),
        };
        let r = c
            .post(format!("{base}{path}"))
            .json(&body)
            .send()
            .await
            .unwrap();
        statuses.push(r.status().as_u16());
    }
    assert!(
        statuses[..4].iter().all(|s| *s != 429),
        "limited too early: {statuses:?}"
    );
    assert_eq!(
        statuses[4], 429,
        "6th credential POST not limited: {statuses:?}"
    );
    // the reads still answer while logins are blocked
    let r = c.get(format!("{base}/auth/realms")).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 200);

    srv.term();
    let _ = std::fs::remove_dir_all(&dir);
}
