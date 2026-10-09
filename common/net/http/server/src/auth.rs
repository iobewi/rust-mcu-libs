//! HTTP credential parsing; authorization policy belongs to each service.

use alloc::string::String;
use core::convert::Infallible;
use picoserve::extract::FromRequestParts;
use picoserve::request::RequestParts;

/// Read a Bearer credential without applying any authorization policy.
pub fn bearer_token(value: Option<&str>) -> &str {
    value.and_then(|v| v.strip_prefix("Bearer ")).unwrap_or("")
}

/// Extract an optional credential from an incoming HTTP request. Absence and
/// malformed headers yield `None`; the consuming service must refuse them.
pub struct Bearer(pub Option<String>);

impl<'r, State> FromRequestParts<'r, State> for Bearer {
    type Rejection = Infallible;

    async fn from_request_parts(
        _state: &'r State,
        request_parts: &RequestParts<'r>,
    ) -> Result<Self, Self::Rejection> {
        let token = request_parts
            .headers()
            .get("authorization")
            .and_then(|value| value.as_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .map(String::from);
        Ok(Self(token))
    }
}
