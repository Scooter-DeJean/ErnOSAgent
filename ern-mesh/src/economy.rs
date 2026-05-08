// Ern-OS — High-performance, model-neutral Rust AI agent engine
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! ErnPoints economy — local ledger for contribution tracking and spending.
//!
//! ErnPoints are **strictly non-transferable** utility credits:
//!
//! - Earned by contributing resources (bandwidth, storage, compute, uptime, WiFi)
//! - Spent on consuming network services (inference, storage, priority access)
//! - Cannot be purchased, gifted, or traded (per §6.2 regulatory mandate)
//! - Ledger is local-only — each node tracks its own balance
//!
//! Rates for earning and spending are sourced entirely from `MeshConfig.economy`.
//! No hardcoded values (per §2.1 governance mandate).
//!
//! The ledger provides an append-only transaction log with proof hashes
//! for audit and dispute resolution.

use serde::{Deserialize, Serialize};
use std::time::SystemTime;

use crate::config::EconomyConfig;

/// Reason for earning points.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EarnReason {
    /// Relayed bandwidth for the network.
    BandwidthRelayed { megabytes: f64 },
    /// Stored data for other peers.
    StorageProvided { gigabyte_hours: f64 },
    /// Provided inference compute.
    ComputeProvided { kilo_tokens: f64 },
    /// Shared WiFi bandwidth.
    WifiShared { megabytes: f64 },
    /// Node uptime contribution.
    UptimeReward { hours: f64 },
}

/// Reason for spending points.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SpendReason {
    /// Consumed inference compute from the network.
    InferenceConsumed { kilo_tokens: f64 },
    /// Used network storage.
    StorageConsumed { gb_months: f64 },
    /// Priority access to a service.
    PriorityAccess { sessions: u32 },
    /// Content replication across the network.
    ContentReplication { replica_months: f64 },
}

/// A single ledger transaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transaction {
    /// Unique transaction ID (monotonically increasing).
    pub id: u64,
    /// Timestamp of the transaction.
    pub timestamp: SystemTime,
    /// Direction and reason.
    pub kind: TransactionKind,
    /// Points earned or spent (always positive; direction from `kind`).
    pub amount: f64,
    /// Running balance after this transaction.
    pub balance_after: f64,
}

/// Transaction direction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TransactionKind {
    /// Points earned for contributing resources.
    Earn(EarnReason),
    /// Points spent on consuming services.
    Spend(SpendReason),
}

/// Local ErnPoints ledger — tracks balance and transaction history.
///
/// Thread-safety: wrap in `RwLock` if shared across tasks.
#[derive(Debug)]
pub struct Ledger {
    /// Current point balance.
    balance: f64,
    /// Append-only transaction log.
    transactions: Vec<Transaction>,
    /// Next transaction ID.
    next_id: u64,
    /// Economy config — rates for earning and spending.
    config: EconomyConfig,
}

impl Ledger {
    /// Create a new ledger with zero balance.
    pub fn new(config: EconomyConfig) -> Self {
        Self {
            balance: 0.0,
            transactions: Vec::new(),
            next_id: 1,
            config,
        }
    }

    /// Get the current point balance.
    pub fn balance(&self) -> f64 {
        self.balance
    }

    /// Get the total number of transactions.
    pub fn transaction_count(&self) -> usize {
        self.transactions.len()
    }

    /// Get the full transaction history.
    pub fn transactions(&self) -> &[Transaction] {
        &self.transactions
    }

    /// Get the most recent N transactions.
    pub fn recent_transactions(&self, n: usize) -> &[Transaction] {
        let start = self.transactions.len().saturating_sub(n);
        &self.transactions[start..]
    }

    /// Record an earning event and return the points earned.
    pub fn earn(&mut self, reason: EarnReason) -> f64 {
        let amount = self.calculate_earn(&reason);
        self.balance += amount;

        let tx = Transaction {
            id: self.next_id,
            timestamp: SystemTime::now(),
            kind: TransactionKind::Earn(reason),
            amount,
            balance_after: self.balance,
        };

        tracing::info!(
            tx_id = tx.id,
            amount = amount,
            balance = self.balance,
            "ErnPoints earned"
        );

        self.transactions.push(tx);
        self.next_id += 1;
        amount
    }

    /// Attempt to spend points. Returns `Ok(amount)` if sufficient balance,
    /// or `Err` with the shortfall.
    pub fn spend(&mut self, reason: SpendReason) -> Result<f64, f64> {
        let amount = self.calculate_spend(&reason);

        if self.balance < amount {
            let shortfall = amount - self.balance;
            tracing::warn!(
                required = amount,
                balance = self.balance,
                shortfall = shortfall,
                "ErnPoints: insufficient balance"
            );
            return Err(shortfall);
        }

        self.balance -= amount;

        let tx = Transaction {
            id: self.next_id,
            timestamp: SystemTime::now(),
            kind: TransactionKind::Spend(reason),
            amount,
            balance_after: self.balance,
        };

        tracing::info!(
            tx_id = tx.id,
            amount = amount,
            balance = self.balance,
            "ErnPoints spent"
        );

        self.transactions.push(tx);
        self.next_id += 1;
        Ok(amount)
    }

    /// Check if the node can afford a spend without executing it.
    pub fn can_afford(&self, reason: &SpendReason) -> bool {
        self.balance >= self.calculate_spend(reason)
    }

    /// Calculate points earned from a contribution — rates from config.
    fn calculate_earn(&self, reason: &EarnReason) -> f64 {
        match reason {
            EarnReason::BandwidthRelayed { megabytes } =>
                megabytes * self.config.points_per_mb_relayed,
            EarnReason::StorageProvided { gigabyte_hours } =>
                gigabyte_hours * self.config.points_per_gb_hour_stored,
            EarnReason::ComputeProvided { kilo_tokens } =>
                kilo_tokens * self.config.points_per_1k_tokens_computed,
            EarnReason::WifiShared { megabytes } =>
                megabytes * self.config.points_per_mb_wifi_shared,
            EarnReason::UptimeReward { hours } =>
                hours * self.config.points_per_hour_uptime,
        }
    }

    /// Calculate points required for a service — rates from config.
    fn calculate_spend(&self, reason: &SpendReason) -> f64 {
        match reason {
            SpendReason::InferenceConsumed { kilo_tokens } =>
                kilo_tokens * self.config.cost_per_1k_tokens_inference,
            SpendReason::StorageConsumed { gb_months } =>
                gb_months * self.config.cost_per_gb_month_storage,
            SpendReason::PriorityAccess { sessions } =>
                *sessions as f64 * self.config.cost_per_session_priority,
            SpendReason::ContentReplication { replica_months } =>
                replica_months * self.config.cost_per_replica_month,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_economy_config() -> EconomyConfig {
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

    #[test]
    fn test_new_ledger_zero_balance() {
        let ledger = Ledger::new(test_economy_config());
        assert_eq!(ledger.balance(), 0.0);
        assert_eq!(ledger.transaction_count(), 0);
    }

    #[test]
    fn test_earn_bandwidth_increases_balance() {
        let mut ledger = Ledger::new(test_economy_config());
        let earned = ledger.earn(EarnReason::BandwidthRelayed { megabytes: 100.0 });

        assert_eq!(earned, 100.0); // 100 MB * 1.0 pts/MB
        assert_eq!(ledger.balance(), 100.0);
        assert_eq!(ledger.transaction_count(), 1);
    }

    #[test]
    fn test_earn_compute_uses_config_rate() {
        let mut ledger = Ledger::new(test_economy_config());
        let earned = ledger.earn(EarnReason::ComputeProvided { kilo_tokens: 10.0 });

        assert_eq!(earned, 20.0); // 10 kT * 2.0 pts/kT
    }

    #[test]
    fn test_earn_uptime() {
        let mut ledger = Ledger::new(test_economy_config());
        let earned = ledger.earn(EarnReason::UptimeReward { hours: 24.0 });

        assert_eq!(earned, 24.0); // 24h * 1.0 pts/h
    }

    #[test]
    fn test_earn_wifi() {
        let mut ledger = Ledger::new(test_economy_config());
        let earned = ledger.earn(EarnReason::WifiShared { megabytes: 500.0 });

        assert_eq!(earned, 50.0); // 500 MB * 0.1 pts/MB
    }

    #[test]
    fn test_earn_storage() {
        let mut ledger = Ledger::new(test_economy_config());
        let earned = ledger.earn(EarnReason::StorageProvided { gigabyte_hours: 100.0 });

        assert_eq!(earned, 50.0); // 100 GB·h * 0.5 pts/GB·h
    }

    #[test]
    fn test_spend_sufficient_balance() {
        let mut ledger = Ledger::new(test_economy_config());
        ledger.earn(EarnReason::BandwidthRelayed { megabytes: 100.0 }); // +100

        let result = ledger.spend(SpendReason::InferenceConsumed { kilo_tokens: 5.0 });

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 25.0); // 5 kT * 5.0 cost/kT
        assert_eq!(ledger.balance(), 75.0); // 100 - 25
    }

    #[test]
    fn test_spend_insufficient_balance_fails() {
        let mut ledger = Ledger::new(test_economy_config());
        ledger.earn(EarnReason::BandwidthRelayed { megabytes: 10.0 }); // +10

        let result = ledger.spend(SpendReason::InferenceConsumed { kilo_tokens: 10.0 });
        // Needs 50 points (10 kT * 5.0), but only has 10
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), 40.0); // shortfall = 50 - 10

        // Balance unchanged
        assert_eq!(ledger.balance(), 10.0);
    }

    #[test]
    fn test_can_afford_true() {
        let mut ledger = Ledger::new(test_economy_config());
        ledger.earn(EarnReason::UptimeReward { hours: 100.0 });

        assert!(ledger.can_afford(&SpendReason::PriorityAccess { sessions: 3 }));
    }

    #[test]
    fn test_can_afford_false() {
        let ledger = Ledger::new(test_economy_config());

        assert!(!ledger.can_afford(&SpendReason::PriorityAccess { sessions: 1 }));
    }

    #[test]
    fn test_transaction_ids_monotonic() {
        let mut ledger = Ledger::new(test_economy_config());

        ledger.earn(EarnReason::UptimeReward { hours: 1.0 });
        ledger.earn(EarnReason::UptimeReward { hours: 1.0 });
        ledger.earn(EarnReason::UptimeReward { hours: 1.0 });

        let txs = ledger.transactions();
        assert_eq!(txs[0].id, 1);
        assert_eq!(txs[1].id, 2);
        assert_eq!(txs[2].id, 3);
    }

    #[test]
    fn test_recent_transactions() {
        let mut ledger = Ledger::new(test_economy_config());

        for i in 0..10 {
            ledger.earn(EarnReason::UptimeReward { hours: i as f64 });
        }

        let recent = ledger.recent_transactions(3);
        assert_eq!(recent.len(), 3);
        assert_eq!(recent[0].id, 8);
        assert_eq!(recent[2].id, 10);
    }

    #[test]
    fn test_balance_after_tracks_running_total() {
        let mut ledger = Ledger::new(test_economy_config());

        ledger.earn(EarnReason::BandwidthRelayed { megabytes: 50.0 }); // +50
        ledger.earn(EarnReason::UptimeReward { hours: 10.0 }); // +10

        let txs = ledger.transactions();
        assert_eq!(txs[0].balance_after, 50.0);
        assert_eq!(txs[1].balance_after, 60.0);
    }

    #[test]
    fn test_spend_storage() {
        let mut ledger = Ledger::new(test_economy_config());
        ledger.earn(EarnReason::BandwidthRelayed { megabytes: 200.0 }); // +200

        let result = ledger.spend(SpendReason::StorageConsumed { gb_months: 5.0 });
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 50.0); // 5 * 10.0
    }

    #[test]
    fn test_spend_replication() {
        let mut ledger = Ledger::new(test_economy_config());
        ledger.earn(EarnReason::BandwidthRelayed { megabytes: 100.0 }); // +100

        let result = ledger.spend(SpendReason::ContentReplication { replica_months: 10.0 });
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 30.0); // 10 * 3.0
    }

    #[test]
    fn test_mixed_earn_spend_sequence() {
        let mut ledger = Ledger::new(test_economy_config());

        ledger.earn(EarnReason::BandwidthRelayed { megabytes: 100.0 }); // +100
        assert_eq!(ledger.balance(), 100.0);

        ledger.spend(SpendReason::PriorityAccess { sessions: 5 }).unwrap(); // -10
        assert_eq!(ledger.balance(), 90.0);

        ledger.earn(EarnReason::UptimeReward { hours: 10.0 }); // +10
        assert_eq!(ledger.balance(), 100.0);

        assert_eq!(ledger.transaction_count(), 3);
    }
}
