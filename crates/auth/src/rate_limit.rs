use axum::extract::ConnectInfo;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::{extract::Request, extract::State};
use governor::clock::DefaultClock;
use governor::state::keyed::DefaultKeyedStateStore;
use governor::{Quota, RateLimiter};
use std::net::{IpAddr, SocketAddr};
use std::num::NonZeroU32;
use std::sync::Arc;

/// 10 tentatives par minute par adresse IP.
const LOGIN_PER_MINUTE: u32 = 10;

pub type AuthRateLimiter =
    Arc<RateLimiter<IpAddr, DefaultKeyedStateStore<IpAddr>, DefaultClock>>;

pub fn new_auth_rate_limiter() -> AuthRateLimiter {
    Arc::new(RateLimiter::keyed(Quota::per_minute(
        NonZeroU32::new(LOGIN_PER_MINUTE).expect("constante non nulle"),
    )))
}

pub async fn auth_rate_limit(
    State(limiter): State<AuthRateLimiter>,
    // None when served without into_make_service_with_connect_info (tests).
    connect_info: Option<ConnectInfo<SocketAddr>>,
    request: Request,
    next: Next,
) -> Response {
    if let Some(ConnectInfo(addr)) = connect_info {
        if limiter.check_key(&addr.ip()).is_err() {
            return (StatusCode::TOO_MANY_REQUESTS, "Trop de tentatives d'authentification")
                .into_response();
        }
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    #[test]
    fn allows_up_to_quota_then_rejects() {
        let limiter = new_auth_rate_limiter();
        let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));

        for _ in 0..LOGIN_PER_MINUTE {
            assert!(limiter.check_key(&ip).is_ok(), "doit etre accepte dans le quota");
        }

        assert!(limiter.check_key(&ip).is_err(), "doit rejeter apres epuisement du quota");
    }

    #[test]
    fn isolates_per_ip() {
        let limiter = new_auth_rate_limiter();
        let ip_a = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let ip_b = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));

        for _ in 0..LOGIN_PER_MINUTE {
            let _ = limiter.check_key(&ip_a);
        }

        assert!(limiter.check_key(&ip_a).is_err(), "ip_a epuisee");
        assert!(limiter.check_key(&ip_b).is_ok(), "ip_b non affectee");
    }
}
