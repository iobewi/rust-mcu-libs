//! Shared JSON response mechanics. The service using them decides which
//! status and body its API returns.

use alloc::string::String;
use picoserve::response::{ContentBody, ContentHeaders, Response, StatusCode};

pub type JsonResponse = Response<ContentHeaders, ContentBody<String>>;

pub fn json_ok(body: String) -> JsonResponse {
    Response::ok(body).with_content_type("application/json")
}

pub fn json_error(status: StatusCode, body: &str) -> JsonResponse {
    Response::new(status, String::from(body)).with_content_type("application/json")
}
