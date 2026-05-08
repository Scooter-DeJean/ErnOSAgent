// Ern-OS — High-performance, model-neutral Rust AI agent engine
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! Tests for mesh protocol — signing, verification, serialisation, replay.

#[cfg(test)]
mod tests {
    use crate::protocol::*;

    /// Generate a fresh keypair and PeerId for testing.
    fn test_identity() -> (libp2p_identity::Keypair, libp2p_identity::PeerId) {
        let kp = libp2p_identity::Keypair::generate_ed25519();
        let pid = libp2p_identity::PeerId::from_public_key(&kp.public());
        (kp, pid)
    }

    #[test]
    fn test_create_signed_envelope() {
        let (kp, pid) = test_identity();
        let envelope = create_signed_envelope(
            &kp, &pid, 1,
            MessagePayload::Ping { nonce: 42 },
        ).expect("Must create signed envelope");

        assert_eq!(envelope.body.sender, pid.to_base58());
        assert_eq!(envelope.body.sequence, 1);
        assert!(!envelope.signature.is_empty());
    }

    #[test]
    fn test_verify_valid_signature() {
        let (kp, pid) = test_identity();
        let envelope = create_signed_envelope(
            &kp, &pid, 1,
            MessagePayload::Ping { nonce: 42 },
        ).unwrap();

        let result = verify_envelope(&envelope, &kp.public());
        assert!(result.is_ok(), "Valid signature must verify");
    }

    #[test]
    fn test_verify_invalid_signature_fails() {
        let (kp, pid) = test_identity();
        let mut envelope = create_signed_envelope(
            &kp, &pid, 1,
            MessagePayload::Ping { nonce: 42 },
        ).unwrap();

        // Corrupt the signature
        envelope.signature[0] ^= 0xFF;

        let result = verify_envelope(&envelope, &kp.public());
        assert!(result.is_err(), "Corrupted signature must fail verification");
    }

    #[test]
    fn test_verify_wrong_sender_key_fails() {
        let (kp_a, pid_a) = test_identity();
        let (kp_b, _pid_b) = test_identity();

        let envelope = create_signed_envelope(
            &kp_a, &pid_a, 1,
            MessagePayload::Ping { nonce: 42 },
        ).unwrap();

        // Verify with the wrong public key
        let result = verify_envelope(&envelope, &kp_b.public());
        assert!(result.is_err(), "Wrong sender key must fail verification");
    }

    #[test]
    fn test_tampered_body_fails_verification() {
        let (kp, pid) = test_identity();
        let mut envelope = create_signed_envelope(
            &kp, &pid, 1,
            MessagePayload::Ping { nonce: 42 },
        ).unwrap();

        // Tamper with the body after signing
        envelope.body.sequence = 999;

        let result = verify_envelope(&envelope, &kp.public());
        assert!(result.is_err(), "Tampered body must fail verification");
    }

    #[test]
    fn test_serialize_deserialize_roundtrip() {
        let (kp, pid) = test_identity();
        let original = create_signed_envelope(
            &kp, &pid, 1,
            MessagePayload::Ping { nonce: 42 },
        ).unwrap();

        let wire_bytes = serialize_envelope(&original).unwrap();
        let recovered = deserialize_envelope(&wire_bytes, 1048576).unwrap();

        assert_eq!(original, recovered);
    }

    #[test]
    fn test_deserialize_rejects_oversized_payload() {
        let oversized = vec![0u8; 1024 * 1024 + 1]; // 1 MiB + 1 byte
        let result = deserialize_envelope(&oversized, 1024 * 1024); // 1 MiB limit
        assert!(result.is_err(), "Oversized payload must be rejected");

        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("exceeds size limit"),
            "Error must mention size limit: got '{}'",
            err_msg
        );
    }

    #[test]
    fn test_deserialize_rejects_malformed_json() {
        let garbage = b"this is not json";
        let result = deserialize_envelope(garbage, 1048576);
        assert!(result.is_err(), "Malformed JSON must be rejected");
    }

    #[test]
    fn test_sequence_tracker_accepts_increasing() {
        let mut tracker = SequenceTracker::new();

        assert!(tracker.accept("peer_a", 1));
        assert!(tracker.accept("peer_a", 2));
        assert!(tracker.accept("peer_a", 3));
        assert_eq!(tracker.last_sequence("peer_a"), 3);
    }

    #[test]
    fn test_sequence_tracker_rejects_duplicate() {
        let mut tracker = SequenceTracker::new();

        assert!(tracker.accept("peer_a", 5));
        assert!(!tracker.accept("peer_a", 5), "Duplicate must be rejected");
        assert!(!tracker.accept("peer_a", 3), "Older sequence must be rejected");
    }

    #[test]
    fn test_sequence_tracker_independent_per_sender() {
        let mut tracker = SequenceTracker::new();

        assert!(tracker.accept("peer_a", 1));
        assert!(tracker.accept("peer_b", 1), "Different sender, same seq must be accepted");
        assert_eq!(tracker.tracked_senders(), 2);
    }

    #[test]
    fn test_sequence_tracker_unknown_sender_returns_zero() {
        let tracker = SequenceTracker::new();
        assert_eq!(tracker.last_sequence("unknown"), 0);
    }

    #[test]
    fn test_contribution_proof_payload() {
        let (kp, pid) = test_identity();
        let payload = MessagePayload::ContributionProof {
            resource_type: "bandwidth".to_string(),
            quantity: 1024.0,
            consumer: "peer_b_id".to_string(),
            period_start_secs: 1000,
            period_end_secs: 2000,
        };

        let envelope = create_signed_envelope(&kp, &pid, 1, payload).unwrap();

        match &envelope.body.payload {
            MessagePayload::ContributionProof { resource_type, quantity, .. } => {
                assert_eq!(resource_type, "bandwidth");
                assert!((quantity - 1024.0).abs() < f64::EPSILON);
            }
            _ => panic!("Expected ContributionProof payload"),
        }
    }

    #[test]
    fn test_topic_message_payload() {
        let (kp, pid) = test_identity();
        let payload = MessagePayload::TopicMessage {
            topic: "ernchat/server1/general".to_string(),
            data: b"hello mesh".to_vec(),
        };

        let envelope = create_signed_envelope(&kp, &pid, 1, payload).unwrap();

        match &envelope.body.payload {
            MessagePayload::TopicMessage { topic, data } => {
                assert_eq!(topic, "ernchat/server1/general");
                assert_eq!(data, b"hello mesh");
            }
            _ => panic!("Expected TopicMessage payload"),
        }
    }

    #[test]
    fn test_envelope_timestamp_is_recent() {
        let (kp, pid) = test_identity();
        let envelope = create_signed_envelope(
            &kp, &pid, 1,
            MessagePayload::Ping { nonce: 1 },
        ).unwrap();

        let now = current_timestamp_secs();
        let delta = now.saturating_sub(envelope.body.timestamp_secs);
        assert!(
            delta < 5,
            "Timestamp must be within 5 seconds of now, got delta={}",
            delta
        );
    }

    #[test]
    fn test_full_sign_verify_deserialize_flow() {
        let (kp, pid) = test_identity();

        // Create and sign
        let envelope = create_signed_envelope(
            &kp, &pid, 42,
            MessagePayload::Pong { nonce: 99 },
        ).unwrap();

        // Serialise to wire
        let wire = serialize_envelope(&envelope).unwrap();

        // Deserialise from wire (with size limit)
        let recovered = deserialize_envelope(&wire, 1048576).unwrap();

        // Verify signature
        verify_envelope(&recovered, &kp.public())
            .expect("Full flow: signature must verify after roundtrip");

        // Check replay protection
        let mut tracker = SequenceTracker::new();
        assert!(tracker.accept(&recovered.body.sender, recovered.body.sequence));
        assert!(!tracker.accept(&recovered.body.sender, recovered.body.sequence));
    }
}
