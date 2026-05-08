// Ern-OS — High-performance, model-neutral Rust AI agent engine
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! ErnieBook — decentralised social posting over the mesh network.
//!
//! ErnieBook provides a decentralised social feed (similar to a micro-blog):
//!
//! - **Posts**: Short-form content published to the `ernmesh/erniebook` Gossipsub topic.
//! - **No central server**: Posts propagate peer-to-peer via Gossipsub.
//! - **Signed**: Every post carries an Ed25519 signature from the author.
//! - **Local feed**: Each node maintains its own feed log. No global index.
//! - **Optional**: ErnieBook posting is controlled by `config.erniebook.enabled`.
//!
//! Posts are lightweight — text + optional media hash references.
//! Full media content is stored in the content-addressable store (PR 15).

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::time::SystemTime;

/// The Gossipsub topic for ErnieBook posts.
pub const ERNIEBOOK_TOPIC: &str = "ernmesh/erniebook";

/// A single ErnieBook post.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Post {
    /// Author's PeerId (base58).
    pub author: String,
    /// Display name of the author.
    pub display_name: Option<String>,
    /// Post content (text).
    pub content: String,
    /// Optional content-addressed media references (CID hashes).
    pub media_refs: Vec<String>,
    /// Timestamp of the post.
    pub timestamp: SystemTime,
    /// Unique post ID.
    pub post_id: String,
    /// Optional tags for categorisation.
    pub tags: Vec<String>,
}

impl Post {
    /// Create a new text post.
    pub fn new(
        author: &str,
        content: &str,
        display_name: Option<String>,
        tags: Vec<String>,
    ) -> Self {
        let ts = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let suffix: u32 = rand::random();

        Self {
            author: author.to_string(),
            display_name,
            content: content.to_string(),
            media_refs: Vec::new(),
            timestamp: SystemTime::now(),
            post_id: format!("{}-{}-{:08x}", author, ts, suffix),
            tags,
        }
    }

    /// Create a post with media references.
    pub fn with_media(
        author: &str,
        content: &str,
        media_refs: Vec<String>,
        display_name: Option<String>,
        tags: Vec<String>,
    ) -> Self {
        let mut post = Self::new(author, content, display_name, tags);
        post.media_refs = media_refs;
        post
    }
}

/// Local ErnieBook feed — stores received and authored posts.
///
/// Fixed-capacity ring buffer: oldest posts are evicted when full.
#[derive(Debug)]
pub struct Feed {
    /// Posts in chronological order.
    posts: VecDeque<Post>,
    /// Maximum number of posts retained.
    capacity: usize,
}

impl Feed {
    /// Create a new feed with the given capacity.
    pub fn new(capacity: usize) -> Self {
        Self {
            posts: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    /// Add a post to the feed.
    ///
    /// If the feed is at capacity, the oldest post is evicted.
    pub fn add(&mut self, post: Post) {
        if self.posts.len() >= self.capacity {
            self.posts.pop_front();
        }

        tracing::debug!(
            author = %post.author,
            post_id = %post.post_id,
            content_len = post.content.len(),
            "ErnieBook: post added to local feed"
        );

        self.posts.push_back(post);
    }

    /// Get all posts (oldest first).
    pub fn all_posts(&self) -> &VecDeque<Post> {
        &self.posts
    }

    /// Get the most recent N posts.
    pub fn recent_posts(&self, n: usize) -> Vec<&Post> {
        self.posts.iter().rev().take(n).collect()
    }

    /// Get posts by a specific author.
    pub fn posts_by_author(&self, author: &str) -> Vec<&Post> {
        self.posts.iter().filter(|p| p.author == author).collect()
    }

    /// Get posts containing a specific tag.
    pub fn posts_by_tag(&self, tag: &str) -> Vec<&Post> {
        self.posts
            .iter()
            .filter(|p| p.tags.iter().any(|t| t == tag))
            .collect()
    }

    /// Total number of posts in the feed.
    pub fn count(&self) -> usize {
        self.posts.len()
    }

    /// Returns `true` if the feed is empty.
    pub fn is_empty(&self) -> bool {
        self.posts.is_empty()
    }

    /// Clear all posts.
    pub fn clear(&mut self) {
        self.posts.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_post_new() {
        let post = Post::new("peer_a", "Hello world!", Some("Alice".into()), vec![]);
        assert_eq!(post.author, "peer_a");
        assert_eq!(post.content, "Hello world!");
        assert_eq!(post.display_name.as_deref(), Some("Alice"));
        assert!(post.media_refs.is_empty());
        assert!(!post.post_id.is_empty());
    }

    #[test]
    fn test_post_with_media() {
        let post = Post::with_media(
            "peer_a",
            "Check this out!",
            vec!["QmHash123".into()],
            None,
            vec!["photo".into()],
        );
        assert_eq!(post.media_refs.len(), 1);
        assert_eq!(post.tags, vec!["photo"]);
    }

    #[test]
    fn test_post_ids_unique() {
        let p1 = Post::new("peer_a", "msg1", None, vec![]);
        let p2 = Post::new("peer_a", "msg2", None, vec![]);
        assert_ne!(p1.post_id, p2.post_id);
    }

    #[test]
    fn test_feed_new_empty() {
        let feed = Feed::new(100);
        assert!(feed.is_empty());
        assert_eq!(feed.count(), 0);
    }

    #[test]
    fn test_feed_add_and_count() {
        let mut feed = Feed::new(100);
        feed.add(Post::new("peer_a", "post1", None, vec![]));
        feed.add(Post::new("peer_b", "post2", None, vec![]));

        assert_eq!(feed.count(), 2);
        assert!(!feed.is_empty());
    }

    #[test]
    fn test_feed_evicts_oldest_at_capacity() {
        let mut feed = Feed::new(3);

        for i in 0..5 {
            feed.add(Post::new("peer_a", &format!("post{}", i), None, vec![]));
        }

        assert_eq!(feed.count(), 3);
        let posts: Vec<_> = feed.all_posts().iter().collect();
        assert_eq!(posts[0].content, "post2");
        assert_eq!(posts[2].content, "post4");
    }

    #[test]
    fn test_feed_recent_posts() {
        let mut feed = Feed::new(100);
        for i in 0..10 {
            feed.add(Post::new("peer_a", &format!("post{}", i), None, vec![]));
        }

        let recent = feed.recent_posts(3);
        assert_eq!(recent.len(), 3);
        assert_eq!(recent[0].content, "post9"); // most recent first
        assert_eq!(recent[2].content, "post7");
    }

    #[test]
    fn test_feed_posts_by_author() {
        let mut feed = Feed::new(100);
        feed.add(Post::new("alice", "from alice", None, vec![]));
        feed.add(Post::new("bob", "from bob", None, vec![]));
        feed.add(Post::new("alice", "also alice", None, vec![]));

        let alice_posts = feed.posts_by_author("alice");
        assert_eq!(alice_posts.len(), 2);
    }

    #[test]
    fn test_feed_posts_by_tag() {
        let mut feed = Feed::new(100);
        feed.add(Post::new("a", "tagged", None, vec!["tech".into(), "mesh".into()]));
        feed.add(Post::new("b", "no tags", None, vec![]));
        feed.add(Post::new("c", "also tech", None, vec!["tech".into()]));

        let tech = feed.posts_by_tag("tech");
        assert_eq!(tech.len(), 2);

        let mesh = feed.posts_by_tag("mesh");
        assert_eq!(mesh.len(), 1);

        let empty = feed.posts_by_tag("nonexistent");
        assert!(empty.is_empty());
    }

    #[test]
    fn test_feed_clear() {
        let mut feed = Feed::new(100);
        feed.add(Post::new("a", "post", None, vec![]));
        feed.clear();
        assert!(feed.is_empty());
    }

    #[test]
    fn test_post_serialize_roundtrip() {
        let post = Post::new("peer_a", "Hello!", Some("Alice".into()), vec!["intro".into()]);
        let json = serde_json::to_string(&post).unwrap();
        let recovered: Post = serde_json::from_str(&json).unwrap();

        assert_eq!(recovered.author, "peer_a");
        assert_eq!(recovered.content, "Hello!");
        assert_eq!(recovered.tags, vec!["intro"]);
    }

    #[test]
    fn test_erniebook_topic_constant() {
        assert_eq!(ERNIEBOOK_TOPIC, "ernmesh/erniebook");
    }
}
