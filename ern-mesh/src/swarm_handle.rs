// Ern-OS — High-performance, model-neutral Rust AI agent engine
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! Swarm command channel — async commands from handlers to the Swarm event loop.
//!
//! The Swarm runs in a background tokio task. HTTP handlers cannot directly
//! access the Swarm. This module provides a typed command channel that
//! handlers use to send publish/subscribe commands to the Swarm.
//!
//! ```text
//! POST handler → SwarmTx::send(Publish{..}) → event loop → gossipsub.publish()
//! ```

/// A command sent from HTTP handlers to the Swarm event loop.
#[derive(Debug)]
pub enum SwarmCommand {
    /// Publish a message to a Gossipsub topic.
    Publish {
        /// The Gossipsub topic string.
        topic: String,
        /// Serialised message bytes.
        data: Vec<u8>,
    },
    /// Subscribe to a Gossipsub topic.
    Subscribe {
        /// The Gossipsub topic string.
        topic: String,
    },
    /// Unsubscribe from a Gossipsub topic.
    Unsubscribe {
        /// The Gossipsub topic string.
        topic: String,
    },
}

/// Sender half — cloned into HTTP handlers / MeshRuntime.
pub type SwarmTx = tokio::sync::mpsc::Sender<SwarmCommand>;

/// Receiver half — owned by the Swarm event loop.
pub type SwarmRx = tokio::sync::mpsc::Receiver<SwarmCommand>;

/// Create a new command channel with a bounded buffer.
pub fn command_channel() -> (SwarmTx, SwarmRx) {
    tokio::sync::mpsc::channel(256)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_command_publish() {
        let (tx, mut rx) = command_channel();
        tx.send(SwarmCommand::Publish {
            topic: "ernmesh/chat/general".into(),
            data: b"hello".to_vec(),
        }).await.unwrap();

        let cmd = rx.recv().await.unwrap();
        match cmd {
            SwarmCommand::Publish { topic, data } => {
                assert_eq!(topic, "ernmesh/chat/general");
                assert_eq!(data, b"hello");
            }
            _ => panic!("Expected Publish"),
        }
    }

    #[tokio::test]
    async fn test_command_subscribe() {
        let (tx, mut rx) = command_channel();
        tx.send(SwarmCommand::Subscribe {
            topic: "ernmesh/erniebook".into(),
        }).await.unwrap();

        let cmd = rx.recv().await.unwrap();
        match cmd {
            SwarmCommand::Subscribe { topic } => {
                assert_eq!(topic, "ernmesh/erniebook");
            }
            _ => panic!("Expected Subscribe"),
        }
    }
}
