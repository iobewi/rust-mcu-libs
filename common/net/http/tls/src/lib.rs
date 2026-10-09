#![no_std]

//! HTTPS provisioning API for the portable TLS identity and trust service.
//!
//! HTTP framework glue lives here, not in `iobewi-tls-service`, so the TLS
//! service stays free of any HTTP type.
//!
//! The application mounts these relative paths under its own API prefix and
//! attaches the handlers to its shared HTTP router. It
//! supplies authorization plus the platform-backed durable TLS service.
//! The caller must expose the routes only on an authenticated TLS listener.

extern crate alloc;
#[cfg(test)]
extern crate std;

use alloc::string::String;
use iobewi_http_server::json::{json_error, json_ok, JsonResponse};
use iobewi_http_server::response::StatusCode;
use serde::Deserialize;

use iobewi_tls_service::SaveCertError;

pub const CERT_PATH: &str = "/tls/cert";
pub const CA_PATH: &str = "/tls/ca";

#[derive(Deserialize)]
struct CertBody {
    cert_pem: String,
    key_pem: String,
}

#[derive(Deserialize)]
struct CaBody {
    ca_pem: String,
}

/// The application supplies its existing Bearer policy; the platform
/// supplies certificate validation and persistent storage through the TLS
/// service. No hardware or application configuration type enters this API.
#[allow(async_fn_in_trait)]
pub trait ProvisioningBackend {
    async fn authorize(&self, token: &str) -> bool;
    async fn save_cert(&self, cert_pem: &str, key_pem: &str) -> Result<(), SaveCertError>;
    async fn save_ca(&self, ca_pem: &str) -> Result<(), SaveCertError>;
}

pub async fn cert_response<B: ProvisioningBackend>(backend: &B, bearer: &str, body: &str) -> JsonResponse {
    if !backend.authorize(bearer).await {
        return json_error(StatusCode::UNAUTHORIZED, "{\"error\":\"unauthorized\"}");
    }
    let Ok(request) = serde_json::from_str::<CertBody>(body) else {
        return json_error(StatusCode::BAD_REQUEST, "{\"error\":\"missing_cert_or_key\"}");
    };
    match backend.save_cert(&request.cert_pem, &request.key_pem).await {
        Ok(()) => json_ok(String::from("{\"status\":\"saved\"}")),
        Err(SaveCertError::Invalid) => json_error(StatusCode::BAD_REQUEST, "{\"error\":\"invalid_certificate\"}"),
        Err(SaveCertError::Mismatch) => json_error(StatusCode::BAD_REQUEST, "{\"error\":\"cert_key_mismatch\"}"),
        Err(SaveCertError::Storage) => json_error(StatusCode::INTERNAL_SERVER_ERROR, "{\"error\":\"nvs_write_failed\"}"),
    }
}

pub async fn ca_response<B: ProvisioningBackend>(backend: &B, bearer: &str, body: &str) -> JsonResponse {
    if !backend.authorize(bearer).await {
        return json_error(StatusCode::UNAUTHORIZED, "{\"error\":\"unauthorized\"}");
    }
    let Ok(request) = serde_json::from_str::<CaBody>(body) else {
        return json_error(StatusCode::BAD_REQUEST, "{\"error\":\"missing_ca\"}");
    };
    match backend.save_ca(&request.ca_pem).await {
        Ok(()) => json_ok(String::from("{\"status\":\"saved\"}")),
        Err(SaveCertError::Storage) => json_error(StatusCode::INTERNAL_SERVER_ERROR, "{\"error\":\"nvs_write_failed\"}"),
        Err(SaveCertError::Invalid | SaveCertError::Mismatch) =>
            json_error(StatusCode::BAD_REQUEST, "{\"error\":\"invalid_certificate\"}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::rc::Rc;
    use alloc::string::ToString;
    use alloc::vec::Vec;
    use core::cell::RefCell;
    use core::future::Future;
    use core::task::{Context, Poll, Waker};

    fn block_on<F: Future>(f: F) -> F::Output {
        let mut f = core::pin::pin!(f);
        let mut cx = Context::from_waker(Waker::noop());
        loop {
            if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
                return v;
            }
        }
    }

    #[derive(Clone)]
    struct Backend {
        allow: bool,
        result: Result<(), SaveCertError>,
        saved: Rc<RefCell<Vec<(&'static str, alloc::vec::Vec<alloc::string::String>)>>>,
    }

    impl ProvisioningBackend for Backend {
        async fn authorize(&self, token: &str) -> bool {
            self.allow && token == "t"
        }
        async fn save_cert(&self, cert: &str, key: &str) -> Result<(), SaveCertError> {
            self.saved.borrow_mut().push(("cert", alloc::vec![cert.to_string(), key.to_string()]));
            self.result.clone()
        }
        async fn save_ca(&self, ca: &str) -> Result<(), SaveCertError> {
            self.saved.borrow_mut().push(("ca", alloc::vec![ca.to_string()]));
            self.result.clone()
        }
    }

    fn backend(allow: bool, result: Result<(), SaveCertError>) -> Backend {
        Backend { allow, result, saved: Rc::default() }
    }

    #[test]
    fn unauthorized_requests_never_reach_the_backend() {
        let b = backend(true, Ok(()));
        block_on(cert_response(&b, "wrong", r#"{"cert_pem":"c","key_pem":"k"}"#));
        block_on(ca_response(&b, "", r#"{"ca_pem":"x"}"#));
        assert!(b.saved.borrow().is_empty());
    }

    #[test]
    fn malformed_bodies_are_rejected_before_saving() {
        let b = backend(true, Ok(()));
        block_on(cert_response(&b, "t", r#"{"cert_pem":"c"}"#));
        block_on(ca_response(&b, "t", "not json"));
        assert!(b.saved.borrow().is_empty());
    }

    #[test]
    fn valid_bodies_are_passed_through_verbatim() {
        let b = backend(true, Ok(()));
        block_on(cert_response(&b, "t", r#"{"cert_pem":"CERT","key_pem":"KEY"}"#));
        block_on(ca_response(&b, "t", r#"{"ca_pem":"CA"}"#));
        let saved = b.saved.borrow();
        assert_eq!(saved[0], ("cert", alloc::vec!["CERT".to_string(), "KEY".to_string()]));
        assert_eq!(saved[1], ("ca", alloc::vec!["CA".to_string()]));
    }

    #[test]
    fn every_backend_error_still_produces_a_response() {
        for err in [SaveCertError::Invalid, SaveCertError::Mismatch, SaveCertError::Storage] {
            let b = backend(true, Err(err));
            block_on(cert_response(&b, "t", r#"{"cert_pem":"c","key_pem":"k"}"#));
            block_on(ca_response(&b, "t", r#"{"ca_pem":"x"}"#));
            assert_eq!(b.saved.borrow().len(), 2);
        }
    }
}
