//! Tests for mesh infrastructure service handlers.

use super::*;

#[test]
fn test_validate_peer_id_valid() {
    assert!(validate_peer_id("12D3KooWAbcDef").is_ok());
}

#[test]
fn test_validate_peer_id_empty() {
    assert!(validate_peer_id("").is_err());
}

#[test]
fn test_validate_peer_id_too_long() {
    let long = "a".repeat(129);
    assert!(validate_peer_id(&long).is_err());
}

#[test]
fn test_validate_peer_id_at_limit() {
    let exact = "a".repeat(128);
    assert!(validate_peer_id(&exact).is_ok());
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
