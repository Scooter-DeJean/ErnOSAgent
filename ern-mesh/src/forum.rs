// Ern-OS — High-performance, model-neutral Rust AI agent engine
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! Mesh forums — decentralised threaded discussions (Reddit-like).
//!
//! Forums provide structured, topic-based discussions:
//!
//! - **Communities**: User-created topic groups (like subreddits).
//! - **Threads**: Posts within a community, with nested replies.
//! - **Voting**: Upvote/downvote on posts and replies.
//! - **Moderation**: Community creator sets rules; peers can flag content.
//! - **Content-addressed**: Thread content stored via content store CIDs
//!   for persistence and replication.
//!
//! Forums are distributed via the `ernmesh/forum/{community}` Gossipsub topic.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::time::SystemTime;

/// A forum community.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Community {
    /// Unique community name (lowercase, no spaces).
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// PeerId of the creator/moderator.
    pub creator: String,
    /// When the community was created.
    pub created_at: SystemTime,
    /// Community rules (displayed to users).
    pub rules: Vec<String>,
}

impl Community {
    /// Create a new community.
    pub fn new(name: &str, description: &str, creator: &str) -> Self {
        Self {
            name: name.to_lowercase(),
            description: description.to_string(),
            creator: creator.to_string(),
            created_at: SystemTime::now(),
            rules: Vec::new(),
        }
    }

    /// Add a rule.
    pub fn add_rule(&mut self, rule: &str) {
        self.rules.push(rule.to_string());
    }

    /// The Gossipsub topic for this community.
    pub fn topic(&self) -> String {
        format!("ernmesh/forum/{}", self.name)
    }
}

/// A forum post (thread starter or reply).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForumPost {
    /// Unique post ID.
    pub post_id: String,
    /// Community this post belongs to.
    pub community: String,
    /// Author's PeerId.
    pub author: String,
    /// Post title (only for thread starters).
    pub title: Option<String>,
    /// Post body.
    pub body: String,
    /// Parent post ID (None = thread starter, Some = reply).
    pub parent_id: Option<String>,
    /// When the post was created.
    pub created_at: SystemTime,
    /// Upvote count.
    pub upvotes: u32,
    /// Downvote count.
    pub downvotes: u32,
    /// Optional content CID for media attachments.
    pub attachment_cid: Option<String>,
}

impl ForumPost {
    /// Create a new thread-starting post.
    pub fn new_thread(community: &str, author: &str, title: &str, body: &str) -> Self {
        let suffix: u32 = rand::random();
        Self {
            post_id: format!("{}-{:08x}", author, suffix),
            community: community.to_string(),
            author: author.to_string(),
            title: Some(title.to_string()),
            body: body.to_string(),
            parent_id: None,
            created_at: SystemTime::now(),
            upvotes: 0,
            downvotes: 0,
            attachment_cid: None,
        }
    }

    /// Create a reply to an existing post.
    pub fn new_reply(community: &str, author: &str, parent_id: &str, body: &str) -> Self {
        let suffix: u32 = rand::random();
        Self {
            post_id: format!("{}-{:08x}", author, suffix),
            community: community.to_string(),
            author: author.to_string(),
            title: None,
            body: body.to_string(),
            parent_id: Some(parent_id.to_string()),
            created_at: SystemTime::now(),
            upvotes: 0,
            downvotes: 0,
            attachment_cid: None,
        }
    }

    /// Whether this is a thread starter (not a reply).
    pub fn is_thread_starter(&self) -> bool {
        self.parent_id.is_none()
    }

    /// Upvote this post.
    pub fn upvote(&mut self) {
        self.upvotes += 1;
    }

    /// Downvote this post.
    pub fn downvote(&mut self) {
        self.downvotes += 1;
    }

    /// Net score (upvotes - downvotes).
    pub fn score(&self) -> i64 {
        self.upvotes as i64 - self.downvotes as i64
    }

    /// Attach a content CID.
    pub fn attach(&mut self, cid: &str) {
        self.attachment_cid = Some(cid.to_string());
    }
}

/// Local forum store — tracks communities and posts.
#[derive(Debug, Default)]
pub struct ForumStore {
    /// Communities indexed by name.
    communities: HashMap<String, Community>,
    /// Posts indexed by community name → ordered posts.
    posts: HashMap<String, VecDeque<ForumPost>>,
    /// Maximum posts per community.
    max_posts_per_community: usize,
}

impl ForumStore {
    /// Create a new forum store.
    pub fn new(max_posts_per_community: usize) -> Self {
        Self {
            communities: HashMap::new(),
            posts: HashMap::new(),
            max_posts_per_community,
        }
    }

    /// Register a community.
    pub fn create_community(&mut self, community: Community) -> anyhow::Result<()> {
        if community.name.is_empty() {
            anyhow::bail!("Community name cannot be empty");
        }
        if self.communities.contains_key(&community.name) {
            anyhow::bail!("Community '{}' already exists", community.name);
        }

        tracing::info!(
            name = %community.name,
            creator = %community.creator,
            "Forum community created"
        );

        self.communities.insert(community.name.clone(), community);
        Ok(())
    }

    /// Get a community by name.
    pub fn get_community(&self, name: &str) -> Option<&Community> {
        self.communities.get(name)
    }

    /// Add a post to a community.
    pub fn add_post(&mut self, post: ForumPost) -> anyhow::Result<()> {
        if !self.communities.contains_key(&post.community) {
            anyhow::bail!("Community '{}' not found", post.community);
        }

        let posts = self.posts
            .entry(post.community.clone())
            .or_insert_with(VecDeque::new);

        if posts.len() >= self.max_posts_per_community {
            posts.pop_front();
        }

        tracing::info!(
            community = %post.community,
            author = %post.author,
            is_reply = post.parent_id.is_some(),
            "Forum post added"
        );

        posts.push_back(post);
        Ok(())
    }

    /// Get all posts in a community.
    pub fn posts_in(&self, community: &str) -> Vec<&ForumPost> {
        self.posts.get(community)
            .map(|d| d.iter().collect())
            .unwrap_or_default()
    }

    /// Get thread-starting posts in a community (no replies).
    pub fn threads_in(&self, community: &str) -> Vec<&ForumPost> {
        self.posts.get(community)
            .map(|posts| posts.iter().filter(|p| p.is_thread_starter()).collect())
            .unwrap_or_default()
    }

    /// Get replies to a specific post.
    pub fn replies_to(&self, community: &str, post_id: &str) -> Vec<&ForumPost> {
        self.posts.get(community)
            .map(|posts| {
                posts.iter()
                    .filter(|p| p.parent_id.as_deref() == Some(post_id))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get a mutable post by ID.
    pub fn get_post_mut(&mut self, community: &str, post_id: &str) -> Option<&mut ForumPost> {
        self.posts.get_mut(community)
            .and_then(|posts| posts.iter_mut().find(|p| p.post_id == post_id))
    }

    /// Total posts across all communities.
    pub fn total_post_count(&self) -> usize {
        self.posts.values().map(|p| p.len()).sum()
    }

    /// Number of communities.
    pub fn community_count(&self) -> usize {
        self.communities.len()
    }

    /// List all community names.
    pub fn list_communities(&self) -> Vec<&str> {
        self.communities.keys().map(|s| s.as_str()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_store() -> ForumStore {
        let mut store = ForumStore::new(100);
        store.create_community(Community::new("general", "A general discussion", "alice")).unwrap();
        store
    }

    #[test]
    fn test_create_community() {
        let store = make_store();
        assert_eq!(store.community_count(), 1);
        assert!(store.get_community("general").is_some());
    }

    #[test]
    fn test_duplicate_community_fails() {
        let mut store = make_store();
        let result = store.create_community(Community::new("general", "Dup", "bob"));
        assert!(result.is_err());
    }

    #[test]
    fn test_empty_community_name_fails() {
        let mut store = ForumStore::new(100);
        let result = store.create_community(Community::new("", "Empty", "alice"));
        assert!(result.is_err());
    }

    #[test]
    fn test_add_thread() {
        let mut store = make_store();
        let post = ForumPost::new_thread("general", "alice", "Hello World", "First post!");
        store.add_post(post).unwrap();

        assert_eq!(store.total_post_count(), 1);
        assert_eq!(store.threads_in("general").len(), 1);
    }

    #[test]
    fn test_add_reply() {
        let mut store = make_store();
        let thread = ForumPost::new_thread("general", "alice", "Question", "How?");
        let tid = thread.post_id.clone();
        store.add_post(thread).unwrap();

        let reply = ForumPost::new_reply("general", "bob", &tid, "Like this!");
        store.add_post(reply).unwrap();

        assert_eq!(store.total_post_count(), 2);
        assert_eq!(store.threads_in("general").len(), 1);
        assert_eq!(store.replies_to("general", &tid).len(), 1);
    }

    #[test]
    fn test_post_to_unknown_community_fails() {
        let mut store = ForumStore::new(100);
        let post = ForumPost::new_thread("nonexistent", "alice", "T", "B");
        assert!(store.add_post(post).is_err());
    }

    #[test]
    fn test_upvote_downvote() {
        let mut post = ForumPost::new_thread("c", "a", "T", "B");
        post.upvote();
        post.upvote();
        post.downvote();

        assert_eq!(post.score(), 1);
        assert_eq!(post.upvotes, 2);
        assert_eq!(post.downvotes, 1);
    }

    #[test]
    fn test_is_thread_starter() {
        let thread = ForumPost::new_thread("c", "a", "T", "B");
        let reply = ForumPost::new_reply("c", "b", "parent-1", "R");

        assert!(thread.is_thread_starter());
        assert!(!reply.is_thread_starter());
    }

    #[test]
    fn test_community_topic() {
        let c = Community::new("rust", "Rust Discussion", "alice");
        assert_eq!(c.topic(), "ernmesh/forum/rust");
    }

    #[test]
    fn test_community_rules() {
        let mut c = Community::new("c", "d", "a");
        c.add_rule("Be respectful");
        c.add_rule("No spam");
        assert_eq!(c.rules.len(), 2);
    }

    #[test]
    fn test_post_attach() {
        let mut post = ForumPost::new_thread("c", "a", "T", "B");
        assert!(post.attachment_cid.is_none());

        post.attach("QmHash123");
        assert_eq!(post.attachment_cid.as_deref(), Some("QmHash123"));
    }

    #[test]
    fn test_eviction_on_overflow() {
        let mut store = ForumStore::new(3);
        store.create_community(Community::new("c", "d", "a")).unwrap();

        for i in 0..5 {
            store.add_post(ForumPost::new_thread("c", "a", &format!("T{}", i), "B")).unwrap();
        }

        assert_eq!(store.total_post_count(), 3);
    }

    #[test]
    fn test_post_serializable() {
        let post = ForumPost::new_thread("c", "alice", "Hello", "World");
        let json = serde_json::to_string(&post).unwrap();
        let recovered: ForumPost = serde_json::from_str(&json).unwrap();
        assert_eq!(recovered.author, "alice");
    }
}
