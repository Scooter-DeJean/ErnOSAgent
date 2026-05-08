// Ern-OS — High-performance, model-neutral Rust AI agent engine
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! Accounting integration — auto-earn/spend on mesh resource events.
//!
//! This module bridges the gap between raw mesh events (relay usage,
//! content storage, compute provision) and the ErnPoints ledger.
//!
//! ## How It Works
//!
//! ```text
//! RelayPool session → Accountant::record_relay() → Ledger::earn(BandwidthRelayed)
//! ContentStore::store() → Accountant::record_store() → Ledger::earn(StorageProvided)
//! Swarm uptime tick → Accountant::record_uptime() → Ledger::earn(UptimeReward)
//! Inference request → Accountant::charge_inference() → Ledger::spend(InferenceConsumed)
//! ```
//!
//! The accountant does NOT mutate the relay pool or content store.
//! It only reads usage metrics and writes to the ledger.

use std::sync::Arc;
use tokio::sync::RwLock;

use crate::economy::{EarnReason, Ledger, SpendReason};

/// The mesh accountant — records contributions and charges consumption.
pub struct Accountant {
    /// The ErnPoints ledger.
    ledger: Arc<RwLock<Ledger>>,
}

impl Accountant {
    /// Create a new accountant wrapping the given ledger.
    pub fn new(ledger: Arc<RwLock<Ledger>>) -> Self {
        tracing::info!("Mesh accountant initialised");
        Self { ledger }
    }

    /// Record bandwidth relayed for another peer.
    pub async fn record_relay(&self, megabytes: f64) -> f64 {
        let earned = self.ledger.write().await
            .earn(EarnReason::BandwidthRelayed { megabytes });

        tracing::debug!(
            mb = megabytes,
            earned = earned,
            "Accountant: bandwidth relay recorded"
        );

        earned
    }

    /// Record storage contribution (content pinning duration).
    pub async fn record_storage(&self, gigabyte_hours: f64) -> f64 {
        let earned = self.ledger.write().await
            .earn(EarnReason::StorageProvided { gigabyte_hours });

        tracing::debug!(
            gb_hours = gigabyte_hours,
            earned = earned,
            "Accountant: storage contribution recorded"
        );

        earned
    }

    /// Record compute contribution (inference tokens provided).
    pub async fn record_compute(&self, kilo_tokens: f64) -> f64 {
        let earned = self.ledger.write().await
            .earn(EarnReason::ComputeProvided { kilo_tokens });

        tracing::debug!(
            kt = kilo_tokens,
            earned = earned,
            "Accountant: compute contribution recorded"
        );

        earned
    }

    /// Record WiFi bandwidth shared.
    pub async fn record_wifi(&self, megabytes: f64) -> f64 {
        let earned = self.ledger.write().await
            .earn(EarnReason::WifiShared { megabytes });

        tracing::debug!(
            mb = megabytes,
            earned = earned,
            "Accountant: WiFi sharing recorded"
        );

        earned
    }

    /// Record uptime contribution.
    pub async fn record_uptime(&self, hours: f64) -> f64 {
        let earned = self.ledger.write().await
            .earn(EarnReason::UptimeReward { hours });

        tracing::debug!(
            hours = hours,
            earned = earned,
            "Accountant: uptime reward recorded"
        );

        earned
    }

    /// Charge for inference consumption.
    ///
    /// Returns `Ok(cost)` if sufficient balance, or `Err(shortfall)`.
    pub async fn charge_inference(&self, kilo_tokens: f64) -> Result<f64, f64> {
        self.ledger.write().await
            .spend(SpendReason::InferenceConsumed { kilo_tokens })
    }

    /// Charge for storage consumption.
    pub async fn charge_storage(&self, gb_months: f64) -> Result<f64, f64> {
        self.ledger.write().await
            .spend(SpendReason::StorageConsumed { gb_months })
    }

    /// Charge for priority access.
    pub async fn charge_priority(&self, sessions: u32) -> Result<f64, f64> {
        self.ledger.write().await
            .spend(SpendReason::PriorityAccess { sessions })
    }

    /// Check if the node can afford an inference request.
    pub async fn can_afford_inference(&self, kilo_tokens: f64) -> bool {
        self.ledger.read().await
            .can_afford(&SpendReason::InferenceConsumed { kilo_tokens })
    }

    /// Get the current balance snapshot.
    pub async fn balance(&self) -> f64 {
        self.ledger.read().await.balance()
    }

    /// Get the total transaction count.
    pub async fn transaction_count(&self) -> usize {
        self.ledger.read().await.transaction_count()
    }

    /// Access the underlying ledger.
    pub fn ledger(&self) -> &Arc<RwLock<Ledger>> {
        &self.ledger
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::EconomyConfig;

    fn test_config() -> EconomyConfig {
        EconomyConfig {
            points_per_mb_relayed: 1.0,
            points_per_gb_hour_stored: 0.5,
            points_per_1k_tokens_computed: 2.0,
            points_per_mb_wifi_shared: 0.1,
            points_per_hour_uptime: 1.0,
            chunk_size_bytes: 262144,
            cost_per_1k_tokens_inference: 5.0,
            cost_per_gb_month_storage: 10.0,
            cost_per_session_priority: 2.0,
            cost_per_replica_month: 3.0,
            pinning_bonus_multiplier: 1.0,
        }
    }

    fn make_accountant() -> Accountant {
        Accountant::new(Arc::new(RwLock::new(Ledger::new(test_config()))))
    }

    #[tokio::test]
    async fn test_record_relay() {
        let acc = make_accountant();
        let earned = acc.record_relay(100.0).await;
        assert_eq!(earned, 100.0);
        assert_eq!(acc.balance().await, 100.0);
    }

    #[tokio::test]
    async fn test_record_storage() {
        let acc = make_accountant();
        let earned = acc.record_storage(10.0).await;
        assert_eq!(earned, 5.0); // 10 GB·h * 0.5
        assert_eq!(acc.balance().await, 5.0);
    }

    #[tokio::test]
    async fn test_record_compute() {
        let acc = make_accountant();
        let earned = acc.record_compute(5.0).await;
        assert_eq!(earned, 10.0); // 5 kT * 2.0
    }

    #[tokio::test]
    async fn test_record_wifi() {
        let acc = make_accountant();
        let earned = acc.record_wifi(200.0).await;
        assert_eq!(earned, 20.0); // 200 MB * 0.1
    }

    #[tokio::test]
    async fn test_record_uptime() {
        let acc = make_accountant();
        let earned = acc.record_uptime(24.0).await;
        assert_eq!(earned, 24.0); // 24h * 1.0
    }

    #[tokio::test]
    async fn test_charge_inference_success() {
        let acc = make_accountant();
        acc.record_relay(100.0).await; // +100

        let result = acc.charge_inference(10.0).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 50.0); // 10 kT * 5.0
        assert_eq!(acc.balance().await, 50.0);
    }

    #[tokio::test]
    async fn test_charge_inference_insufficient() {
        let acc = make_accountant();
        acc.record_relay(10.0).await; // +10

        let result = acc.charge_inference(10.0).await; // needs 50
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), 40.0); // shortfall
        assert_eq!(acc.balance().await, 10.0); // unchanged
    }

    #[tokio::test]
    async fn test_can_afford_inference() {
        let acc = make_accountant();
        assert!(!acc.can_afford_inference(1.0).await);

        acc.record_relay(100.0).await;
        assert!(acc.can_afford_inference(10.0).await);
    }

    #[tokio::test]
    async fn test_mixed_earn_spend() {
        let acc = make_accountant();

        acc.record_relay(50.0).await;   // +50
        acc.record_uptime(10.0).await;  // +10
        acc.charge_priority(5).await.unwrap(); // -10 (5 * 2.0)

        assert_eq!(acc.balance().await, 50.0);
        assert_eq!(acc.transaction_count().await, 3);
    }

    #[tokio::test]
    async fn test_charge_storage() {
        let acc = make_accountant();
        acc.record_relay(200.0).await; // +200

        let result = acc.charge_storage(5.0).await;
        assert_eq!(result.unwrap(), 50.0); // 5 * 10.0
    }
}
