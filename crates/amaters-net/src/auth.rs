//! Authentication middleware for AmateRS network layer.
//!
//! Provides a Tower middleware layer for pluggable authentication, a default
//! `BearerTokenValidator` using HMAC-SHA256 JWTs via the `jsonwebtoken` crate,
//! and an `AuthValidator` trait for custom implementations.
//!
//! # Architecture
//!
//! - [`AuthValidator`]: object-safe async trait — implementations validate raw
//!   token bytes and return [`Claims`] on success.
//! - [`AuthMiddlewareLayer<V>`]: implements [`tower_layer::Layer`]; wraps any
//!   inner service with auth enforcement.
//! - [`AuthMiddleware<S, V>`]: implements [`tower_service::Service`]; extracts
//!   the `authorization` header, delegates to the validator, and either inserts
//!   [`Claims`] into request extensions and calls the inner service, or returns
//!   `tonic::Status::unauthenticated(...)`.
//! - [`BearerTokenValidator`]: default validator that parses `Bearer <token>`
//!   and verifies HMAC-SHA256 JWTs.

use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::task::{Context, Poll};

use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode};
use serde::{Deserialize, Serialize};
use tower_layer::Layer;
use tower_service::Service;

// ─── Claims ─────────────────────────────────────────────────────────────────

/// JWT claims inserted into request extensions upon successful authentication.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    /// Subject — typically the user/service identifier.
    pub sub: String,
    /// Expiry as a Unix timestamp (seconds since epoch).
    pub exp: u64,
    /// Roles granted to this subject.
    pub roles: Vec<String>,
}

// ─── AuthError ───────────────────────────────────────────────────────────────

/// Errors produced by the authentication layer.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    /// No `authorization` header was present.
    #[error("missing authorization token")]
    MissingToken,
    /// The token was structurally invalid or its signature did not verify.
    #[error("invalid token: {0}")]
    InvalidToken(String),
    /// The token has passed its expiry time.
    #[error("token has expired")]
    Expired,
    /// The caller lacks sufficient privileges for the requested operation.
    #[error("unauthorized")]
    Unauthorized,
}

impl From<AuthError> for tonic::Status {
    fn from(e: AuthError) -> tonic::Status {
        tonic::Status::unauthenticated(e.to_string())
    }
}

// ─── AuthValidator ───────────────────────────────────────────────────────────

/// Object-safe trait for pluggable token validation.
///
/// Implementations receive the raw token string (already stripped of any
/// scheme prefix) and return parsed [`Claims`] on success.
///
/// # Object-safety
///
/// The associated future is erased behind `Pin<Box<dyn Future + Send + '_>>`
/// to keep the trait object-safe and usable with `dyn AuthValidator`.
pub trait AuthValidator: Send + Sync {
    /// Validate the raw token and return [`Claims`] on success.
    ///
    /// The `token` argument is the literal content of the `Authorization`
    /// header value (e.g. `"Bearer eyJhbG..."`).
    fn validate<'a>(
        &'a self,
        token: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Claims, AuthError>> + Send + 'a>>;
}

// ─── BearerTokenValidator ────────────────────────────────────────────────────

/// Default validator that parses `Bearer <jwt>` and verifies HMAC-SHA256.
///
/// The expected Authorization header value format is:
/// ```text
/// Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9...
/// ```
#[derive(Clone)]
pub struct BearerTokenValidator {
    decoding_key: DecodingKey,
    validation: Validation,
}

impl BearerTokenValidator {
    /// Create a validator that verifies tokens signed with the given HMAC secret.
    ///
    /// Expiry is validated strictly — the leeway is set to zero so tokens
    /// that have passed their `exp` claim are rejected immediately.
    pub fn new(secret: &[u8]) -> Self {
        let mut validation = Validation::new(Algorithm::HS256);
        // Do not require `aud` by default — callers can set this on their own
        // Validation instance if needed.
        validation.validate_aud = false;
        // Strict expiry: reject tokens the moment they expire (no leeway).
        validation.leeway = 0;
        Self {
            decoding_key: DecodingKey::from_secret(secret),
            validation,
        }
    }
}

impl AuthValidator for BearerTokenValidator {
    fn validate<'a>(
        &'a self,
        token: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Claims, AuthError>> + Send + 'a>> {
        // Strip the "Bearer " prefix.
        let jwt = match token.strip_prefix("Bearer ") {
            Some(t) => t.to_owned(),
            None => {
                return Box::pin(async {
                    Err(AuthError::InvalidToken("not a Bearer token".to_string()))
                });
            }
        };

        let decoding_key = self.decoding_key.clone();
        let validation = self.validation.clone();

        Box::pin(async move {
            match decode::<Claims>(&jwt, &decoding_key, &validation) {
                Ok(token_data) => Ok(token_data.claims),
                Err(e) => {
                    use jsonwebtoken::errors::ErrorKind;
                    match e.kind() {
                        ErrorKind::ExpiredSignature => Err(AuthError::Expired),
                        _ => Err(AuthError::InvalidToken(e.to_string())),
                    }
                }
            }
        })
    }
}

// ─── AuthMiddlewareLayer ─────────────────────────────────────────────────────

/// Tower [`Layer`] that wraps a service with authentication enforcement.
///
/// `V` must implement [`AuthValidator`] and also be `Clone + Send + Sync + 'static`
/// so that the produced middleware can be cloned per-request.
#[derive(Clone)]
pub struct AuthMiddlewareLayer<V>
where
    V: AuthValidator + Clone + Send + Sync + 'static,
{
    validator: V,
}

impl<V> AuthMiddlewareLayer<V>
where
    V: AuthValidator + Clone + Send + Sync + 'static,
{
    /// Create a new layer using the given validator.
    pub fn new(validator: V) -> Self {
        Self { validator }
    }
}

impl<S, V> Layer<S> for AuthMiddlewareLayer<V>
where
    V: AuthValidator + Clone + Send + Sync + 'static,
{
    type Service = AuthMiddleware<S, V>;

    fn layer(&self, inner: S) -> Self::Service {
        AuthMiddleware {
            inner,
            validator: self.validator.clone(),
        }
    }
}

// ─── AuthMiddleware ───────────────────────────────────────────────────────────

/// Tower [`Service`] that enforces authentication on every request.
///
/// On each call it:
/// 1. Extracts the `authorization` header (or the tonic metadata key
///    `authorization`) from the request.
/// 2. Calls `V::validate` on its value.
/// 3. On success inserts the returned [`Claims`] into the request extensions
///    and forwards the request to the inner service.
/// 4. On failure returns an immediate `tonic::Status::unauthenticated(...)`.
#[derive(Clone)]
pub struct AuthMiddleware<S, V>
where
    V: AuthValidator + Clone + Send + Sync + 'static,
{
    inner: S,
    validator: V,
}

impl<S, B, ResBody, V> Service<http::Request<B>> for AuthMiddleware<S, V>
where
    S: Service<http::Request<B>, Response = http::Response<ResBody>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    S::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    B: Send + 'static,
    ResBody: Default + Send + 'static,
    V: AuthValidator + Clone + Send + Sync + 'static,
{
    type Response = http::Response<ResBody>;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: http::Request<B>) -> Self::Future {
        // Extract token from "authorization" header.
        let token_result = req
            .headers()
            .get(http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_owned())
            .ok_or(AuthError::MissingToken);

        let validator = self.validator.clone();
        // Take a clone of the inner service for the async block.
        // We need to call `poll_ready` on it, but since we already called it above,
        // we use `std::mem::replace` pattern to avoid double-borrow.
        let mut inner = self.inner.clone();
        std::mem::swap(&mut self.inner, &mut inner);

        Box::pin(async move {
            let token = match token_result {
                Ok(t) => t,
                Err(e) => {
                    return Ok(build_unauthenticated_response(e.to_string()));
                }
            };

            match validator.validate(&token).await {
                Ok(claims) => {
                    req.extensions_mut().insert(claims);
                    inner.call(req).await
                }
                Err(e) => Ok(build_unauthenticated_response(e.to_string())),
            }
        })
    }
}

/// Build an `http::Response` that carries a gRPC `UNAUTHENTICATED` status.
///
/// The response body is default-constructed so callers do not need to
/// specify a body type.
fn build_unauthenticated_response<ResBody: Default>(message: String) -> http::Response<ResBody> {
    let status = tonic::Status::unauthenticated(message);
    let (mut parts, _body) = http::Response::new(ResBody::default()).into_parts();
    parts.status = http::StatusCode::OK; // gRPC always uses 200 HTTP status
    // Encode the gRPC status in trailers-only response headers.
    parts.headers.insert(
        "grpc-status",
        http::HeaderValue::from_str(&(status.code() as i32).to_string())
            .unwrap_or_else(|_| http::HeaderValue::from_static("16")),
    );
    if let Ok(v) = http::HeaderValue::from_str(status.message()) {
        parts.headers.insert("grpc-message", v);
    }
    http::Response::from_parts(parts, ResBody::default())
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
    use std::time::{SystemTime, UNIX_EPOCH};
    use tower_service::Service as _;

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn make_jwt(secret: &[u8], exp_offset_secs: i64) -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before UNIX epoch")
            .as_secs();
        let exp = if exp_offset_secs >= 0 {
            now + exp_offset_secs as u64
        } else {
            now.saturating_sub(exp_offset_secs.unsigned_abs())
        };
        let claims = Claims {
            sub: "test-user".to_string(),
            exp,
            roles: vec!["admin".to_string()],
        };
        encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(secret),
        )
        .expect("JWT encoding should not fail in tests")
    }

    // ── Always-ok validator ───────────────────────────────────────────────────

    #[derive(Clone)]
    struct AlwaysOkValidator;

    impl AuthValidator for AlwaysOkValidator {
        fn validate<'a>(
            &'a self,
            _token: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Claims, AuthError>> + Send + 'a>> {
            Box::pin(async {
                Ok(Claims {
                    sub: "always-ok".to_string(),
                    exp: u64::MAX,
                    roles: vec![],
                })
            })
        }
    }

    // ── Simple echo-body inner service ────────────────────────────────────────

    #[derive(Clone)]
    struct EchoService;

    impl Service<http::Request<String>> for EchoService {
        type Response = http::Response<String>;
        type Error = Box<dyn std::error::Error + Send + Sync>;
        type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

        fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, req: http::Request<String>) -> Self::Future {
            let claims = req.extensions().get::<Claims>().cloned();
            Box::pin(async move {
                let body = match claims {
                    Some(c) => format!("sub={}", c.sub),
                    None => "no-claims".to_string(),
                };
                Ok(http::Response::new(body))
            })
        }
    }

    // ─────────────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_auth_rejects_missing_token() {
        let layer = AuthMiddlewareLayer::new(AlwaysOkValidator);
        let mut svc = layer.layer(EchoService);

        let req = http::Request::builder()
            .body(String::new())
            .expect("request builder should not fail");

        let resp = svc.call(req).await.expect("service call should not error");

        // Expect gRPC UNAUTHENTICATED status code (16) in headers.
        let grpc_status = resp
            .headers()
            .get("grpc-status")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("missing");
        assert_eq!(
            grpc_status, "16",
            "expected grpc-status=16 (UNAUTHENTICATED)"
        );
    }

    #[tokio::test]
    async fn test_auth_accepts_valid_token() {
        let secret = b"supersecret";
        let jwt = make_jwt(secret, 3600);
        let bearer = format!("Bearer {jwt}");

        let layer = AuthMiddlewareLayer::new(BearerTokenValidator::new(secret));
        let mut svc = layer.layer(EchoService);

        let req = http::Request::builder()
            .header(http::header::AUTHORIZATION, &bearer)
            .body(String::new())
            .expect("request builder should not fail");

        let resp = svc.call(req).await.expect("service call should not error");

        // Body should contain the subject from claims.
        assert_eq!(resp.body(), "sub=test-user");
        // No grpc-status header means success (inner service ran).
        assert!(
            resp.headers().get("grpc-status").is_none(),
            "should not have grpc-status on success"
        );
    }

    #[tokio::test]
    async fn test_auth_custom_validator() {
        let layer = AuthMiddlewareLayer::new(AlwaysOkValidator);
        let mut svc = layer.layer(EchoService);

        let req = http::Request::builder()
            .header(http::header::AUTHORIZATION, "Bearer anything")
            .body(String::new())
            .expect("request builder should not fail");

        let resp = svc.call(req).await.expect("service call should not error");

        assert_eq!(resp.body(), "sub=always-ok");
    }

    /// Verify that `BearerTokenValidator` rejects tokens without "Bearer " prefix.
    #[tokio::test]
    async fn test_bearer_validator_rejects_non_bearer_prefix() {
        let validator = BearerTokenValidator::new(b"secret");
        let result = validator.validate("Token abc123").await;
        assert!(
            matches!(result, Err(AuthError::InvalidToken(_))),
            "expected InvalidToken for non-Bearer scheme"
        );
    }

    /// Verify that `BearerTokenValidator` rejects expired JWTs.
    #[tokio::test]
    async fn test_bearer_validator_rejects_expired() {
        let secret = b"expiry-test-secret";
        let jwt = make_jwt(secret, -1); // expired 1 second ago
        let bearer = format!("Bearer {jwt}");

        let validator = BearerTokenValidator::new(secret);
        let result = validator.validate(&bearer).await;
        assert!(
            matches!(result, Err(AuthError::Expired)),
            "expected Expired error for expired JWT, got: {:?}",
            result
        );
    }

    /// Verify that `AuthValidator` is object-safe.
    #[test]
    fn test_auth_validator_is_object_safe() {
        // This should compile: we can store a &dyn AuthValidator.
        let validator = BearerTokenValidator::new(b"test");
        let _dyn_ref: &dyn AuthValidator = &validator;
    }

    /// Compile-time test: AuthMiddlewareLayer can be constructed without the
    /// `compression` feature (just a compile check, no runtime assertions).
    #[test]
    fn test_layer_construction() {
        let _layer: AuthMiddlewareLayer<AlwaysOkValidator> =
            AuthMiddlewareLayer::new(AlwaysOkValidator);
    }
}
