use crate::http::{
    auth, misc_routes::prepare_password_reset_request, rate_limit::PasswordResetRateLimiter,
};
use axum::http::StatusCode;

/// The endpoint is implemented now, so what is worth pinning is that the
/// rate limiter still runs before anything else — it fronts an unauthenticated
/// route that sends mail.
#[tokio::test]
async fn password_reset_rate_limits_before_doing_any_work() -> anyhow::Result<()> {
    let limiter = PasswordResetRateLimiter::new(2);
    let email = prepare_password_reset_request(&limiter, 2, "user@example.com")
        .expect("valid email should pass rate limiting");
    assert_eq!(email, "user@example.com");

    // Second is still within the limit; the third is not.
    prepare_password_reset_request(&limiter, 2, "user@example.com").expect("second is allowed");
    let (status, json) = prepare_password_reset_request(&limiter, 2, "user@example.com")
        .expect_err("third exceeds the limit");
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert!(
        json["error"].as_str().unwrap().contains("rate limit"),
        "{json}"
    );
    Ok(())
}

#[tokio::test]
async fn password_reset_rejects_blank_email() -> anyhow::Result<()> {
    let limiter = PasswordResetRateLimiter::new(2);
    let (status, json) = prepare_password_reset_request(&limiter, 2, "   ").unwrap_err();

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["error"], "email is required");
    Ok(())
}

#[tokio::test]
async fn password_reset_rate_limit_uses_normalized_email() -> anyhow::Result<()> {
    let limiter = PasswordResetRateLimiter::new(2);

    for _ in 0..2 {
        let email = prepare_password_reset_request(&limiter, 2, "  USER@example.com  ")
            .expect("normalized email should be allowed within the rate limit");
        assert_eq!(email, "user@example.com");
    }

    let (status, json) =
        prepare_password_reset_request(&limiter, 2, "user@example.com").unwrap_err();

    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert!(
        json["error"]
            .as_str()
            .unwrap_or("")
            .contains("rate limit exceeded"),
        "expected rate limit error body, got: {json}"
    );
    Ok(())
}

#[tokio::test]
async fn password_reset_exempt_from_auth() -> anyhow::Result<()> {
    assert!(
        auth::is_auth_exempt_path("/auth/reset-password"),
        "password reset endpoint must not require auth"
    );
    Ok(())
}
