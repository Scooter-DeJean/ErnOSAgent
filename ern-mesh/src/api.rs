// Ern-OS — High-performance, model-neutral Rust AI agent engine
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! Mesh API — HTTP/JSON endpoints for the WebUI mesh tab.
//!
//! Provides a REST API surface that the WebUI JavaScript frontend
//! calls to render the mesh dashboard, manage services, and interact
//! with the decentralised platform.
//!
//! ## Endpoints
//!
//! | Method | Path                        | Description                    |
//! |--------|-----------------------------|--------------------------------|
//! | GET    | `/api/mesh/status`          | Runtime snapshot (dashboard)   |
//! | GET    | `/api/mesh/chat/{topic}`    | Chat history for a topic       |
//! | POST   | `/api/mesh/chat/{topic}`    | Send a chat message            |
//! | GET    | `/api/mesh/feed`            | ErnieBook feed                 |
//! | POST   | `/api/mesh/feed`            | Create an ErnieBook post       |
//! | GET    | `/api/mesh/mail/inbox`      | Inbox messages                 |
//! | POST   | `/api/mesh/mail/send`       | Send a mail message            |
//! | GET    | `/api/mesh/forums`          | List communities               |
//! | GET    | `/api/mesh/voice`           | List voice rooms               |
//! | GET    | `/api/mesh/sites`           | List hosted mesh sites         |
//! | GET    | `/api/mesh/reputation`      | Peer reputation leaderboard    |
//!
//! The API layer converts between JSON and Rust types. It does not
//! own any state — all state lives in `MeshRuntime`.

use serde::{Deserialize, Serialize};

/// JSON request body for sending a chat message.
#[derive(Debug, Deserialize)]
pub struct SendChatRequest {
    /// The topic to send to (e.g., "general").
    pub topic: String,
    /// The message content.
    pub content: String,
    /// Optional display name.
    pub display_name: Option<String>,
}

/// JSON request body for creating an ErnieBook post.
#[derive(Debug, Deserialize)]
pub struct CreatePostRequest {
    /// Post content.
    pub content: String,
    /// Optional display name.
    pub display_name: Option<String>,
    /// Tags.
    pub tags: Vec<String>,
}

/// JSON request body for sending a mail message.
#[derive(Debug, Deserialize)]
pub struct SendMailRequest {
    /// Recipient PeerId.
    pub to: String,
    /// Subject line.
    pub subject: String,
    /// Body text.
    pub body: String,
}

/// JSON response for a chat message.
#[derive(Debug, Serialize)]
pub struct ChatMessageResponse {
    pub sender: String,
    pub content: String,
    pub display_name: Option<String>,
    pub encrypted: bool,
    pub timestamp: String,
}

/// JSON response for an ErnieBook post.
#[derive(Debug, Serialize)]
pub struct PostResponse {
    pub author: String,
    pub content: String,
    pub display_name: Option<String>,
    pub tags: Vec<String>,
    pub timestamp: String,
}

/// JSON response for a mail message.
#[derive(Debug, Serialize)]
pub struct MailResponse {
    pub message_id: String,
    pub from: String,
    pub subject: String,
    pub body: String,
    pub read: bool,
    pub timestamp: String,
}

/// JSON response for a forum community.
#[derive(Debug, Serialize)]
pub struct CommunityResponse {
    pub name: String,
    pub description: String,
    pub creator: String,
    pub rules: Vec<String>,
}

/// JSON response for a voice room.
#[derive(Debug, Serialize)]
pub struct VoiceRoomResponse {
    pub room_id: String,
    pub name: String,
    pub creator: String,
    pub participants: usize,
    pub max_participants: u32,
    pub needs_sfu: bool,
}

/// JSON response for a hosted mesh site.
#[derive(Debug, Serialize)]
pub struct SiteResponse {
    pub name: String,
    pub description: String,
    pub version: u64,
    pub file_count: usize,
}

/// JSON response for a peer's reputation.
#[derive(Debug, Serialize)]
pub struct ReputationResponse {
    pub peer_id: String,
    pub score: f64,
    pub positive_events: u64,
    pub negative_events: u64,
    pub is_trusted: bool,
    pub is_hostile: bool,
}

/// Standard API error response.
#[derive(Debug, Serialize)]
pub struct ApiError {
    pub error: String,
    pub code: u16,
}

impl ApiError {
    /// Create a 400 Bad Request error.
    pub fn bad_request(msg: &str) -> Self {
        Self {
            error: msg.to_string(),
            code: 400,
        }
    }

    /// Create a 404 Not Found error.
    pub fn not_found(msg: &str) -> Self {
        Self {
            error: msg.to_string(),
            code: 404,
        }
    }

    /// Create a 500 Internal Server Error.
    pub fn internal(msg: &str) -> Self {
        Self {
            error: msg.to_string(),
            code: 500,
        }
    }
}

/// Standard API success wrapper.
#[derive(Debug, Serialize)]
pub struct ApiResponse<T: Serialize> {
    pub success: bool,
    pub data: T,
}

impl<T: Serialize> ApiResponse<T> {
    /// Wrap data in a success response.
    pub fn ok(data: T) -> Self {
        Self {
            success: true,
            data,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_send_chat_request_deserialize() {
        let json = r#"{"topic":"general","content":"Hello!","display_name":"Alice"}"#;
        let req: SendChatRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.topic, "general");
        assert_eq!(req.content, "Hello!");
        assert_eq!(req.display_name.as_deref(), Some("Alice"));
    }

    #[test]
    fn test_send_chat_request_no_display_name() {
        let json = r#"{"topic":"dev","content":"Hello!"}"#;
        let req: SendChatRequest = serde_json::from_str(json).unwrap();
        assert!(req.display_name.is_none());
    }

    #[test]
    fn test_create_post_request_deserialize() {
        let json = r#"{"content":"My post!","display_name":"Bob","tags":["intro","hello"]}"#;
        let req: CreatePostRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.content, "My post!");
        assert_eq!(req.tags.len(), 2);
    }

    #[test]
    fn test_send_mail_request_deserialize() {
        let json = r#"{"to":"peer_bob","subject":"Hi","body":"How are you?"}"#;
        let req: SendMailRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.to, "peer_bob");
        assert_eq!(req.subject, "Hi");
    }

    #[test]
    fn test_chat_message_response_serialize() {
        let resp = ChatMessageResponse {
            sender: "alice".into(),
            content: "Hello!".into(),
            display_name: Some("Alice".into()),
            encrypted: false,
            timestamp: "2026-01-01T00:00:00Z".into(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"sender\":\"alice\""));
        assert!(json.contains("\"encrypted\":false"));
    }

    #[test]
    fn test_api_error_bad_request() {
        let err = ApiError::bad_request("Invalid topic");
        assert_eq!(err.code, 400);
        let json = serde_json::to_string(&err).unwrap();
        assert!(json.contains("Invalid topic"));
    }

    #[test]
    fn test_api_error_not_found() {
        let err = ApiError::not_found("Room not found");
        assert_eq!(err.code, 404);
    }

    #[test]
    fn test_api_error_internal() {
        let err = ApiError::internal("Database error");
        assert_eq!(err.code, 500);
    }

    #[test]
    fn test_api_response_wrap() {
        let resp = ApiResponse::ok(vec!["a", "b", "c"]);
        assert!(resp.success);
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"success\":true"));
    }

    #[test]
    fn test_voice_room_response_serialize() {
        let resp = VoiceRoomResponse {
            room_id: "r1".into(),
            name: "General".into(),
            creator: "alice".into(),
            participants: 3,
            max_participants: 10,
            needs_sfu: false,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"room_id\":\"r1\""));
    }

    #[test]
    fn test_reputation_response_serialize() {
        let resp = ReputationResponse {
            peer_id: "alice".into(),
            score: 42.5,
            positive_events: 10,
            negative_events: 2,
            is_trusted: true,
            is_hostile: false,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"is_trusted\":true"));
    }

    #[test]
    fn test_community_response_serialize() {
        let resp = CommunityResponse {
            name: "rust".into(),
            description: "Rust discussion".into(),
            creator: "alice".into(),
            rules: vec!["Be nice".into()],
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"name\":\"rust\""));
    }

    #[test]
    fn test_site_response_serialize() {
        let resp = SiteResponse {
            name: "my-blog".into(),
            description: "A blog".into(),
            version: 3,
            file_count: 5,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"version\":3"));
    }
}
