//! Tests for mesh communication service handlers.

use super::*;

#[test]
fn test_validate_name_valid() {
    assert!(validate_name("general").is_ok());
    assert!(validate_name("my-topic_123").is_ok());
}

#[test]
fn test_validate_name_empty() {
    assert!(validate_name("").is_err());
}

#[test]
fn test_validate_name_too_long() {
    let long = "a".repeat(65);
    assert!(validate_name(&long).is_err());
}

#[test]
fn test_validate_name_special_chars() {
    assert!(validate_name("hello world").is_err());
    assert!(validate_name("foo/bar").is_err());
    assert!(validate_name("a@b").is_err());
}

#[test]
fn test_validate_content_valid() {
    assert!(validate_content("Hello!").is_ok());
}

#[test]
fn test_validate_content_empty() {
    assert!(validate_content("").is_err());
}

#[test]
fn test_validate_content_too_long() {
    let long = "a".repeat(4097);
    assert!(validate_content(&long).is_err());
}

#[test]
fn test_validate_content_at_limit() {
    let exact = "a".repeat(4096);
    assert!(validate_content(&exact).is_ok());
}

#[test]
fn test_ts_to_string_produces_digits() {
    let s = ts_to_string(std::time::SystemTime::now());
    assert!(s.chars().all(|c| c.is_ascii_digit()));
    assert!(!s.is_empty());
}

#[test]
fn test_mesh_disabled_returns_503() {
    let (status, _) = mesh_disabled();
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

#[test]
fn test_bad_request_returns_400() {
    let (status, body) = bad_request("test error");
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let v = body.0;
    assert_eq!(v["error"], "test error");
}
