use tracing::{error, info, warn};

/// Circuit breaker that halts trading when loss thresholds are exceeded.
///
/// Two independent trip conditions:
/// 1. **Consecutive failures** — N trades in a row that fail or lose money.
/// 2. **Cumulative loss** — Total net loss exceeds a MIST threshold within a rolling window.
///
/// Once tripped, the breaker enters a cooldown period before allowing trades again.
#[derive(Debug)]
pub struct CircuitBreaker {
    // ── Config ──
    max_consecutive_failures: u32,
    max_cumulative_loss_mist: i64,
    cooldown_ms: u64,

    // ── State ──
    consecutive_failures: u32,
    cumulative_pnl_mist: i64,
    total_trades: u64,
    tripped_at_ms: Option<u64>,
    trip_reason: Option<String>,
}

impl CircuitBreaker {
    /// Create a new circuit breaker with configurable thresholds.
    ///
    /// # Arguments
    /// * `max_consecutive_failures` — Trip after this many consecutive losing/failed trades
    /// * `max_cumulative_loss_mist` — Trip when cumulative loss exceeds this (positive value, e.g. 500_000_000 = 0.5 SUI)
    /// * `cooldown_ms` — How long to stay tripped before auto-resetting (ms)
    pub fn new(
        max_consecutive_failures: u32,
        max_cumulative_loss_mist: i64,
        cooldown_ms: u64,
    ) -> Self {
        Self {
            max_consecutive_failures,
            max_cumulative_loss_mist,
            cooldown_ms,
            consecutive_failures: 0,
            cumulative_pnl_mist: 0,
            total_trades: 0,
            tripped_at_ms: None,
            trip_reason: None,
        }
    }

    /// Create with sensible defaults: 5 consecutive failures, 1 SUI cumulative loss, 60s cooldown.
    pub fn default_config() -> Self {
        Self::new(5, 1_000_000_000, 60_000)
    }

    /// Check if trading is currently allowed.
    /// If the breaker is tripped but cooldown has elapsed, it auto-resets.
    pub fn is_trading_allowed(&mut self, now_ms: u64) -> bool {
        if let Some(tripped_at) = self.tripped_at_ms {
            let elapsed = now_ms.saturating_sub(tripped_at);
            if elapsed >= self.cooldown_ms {
                info!(
                    cooldown_ms = %self.cooldown_ms,
                    "Circuit breaker cooldown elapsed — resetting"
                );
                self.reset();
                true
            } else {
                let remaining = self.cooldown_ms - elapsed;
                warn!(
                    remaining_ms = %remaining,
                    reason = ?self.trip_reason,
                    "Circuit breaker active — trading paused"
                );
                false
            }
        } else {
            true
        }
    }

    /// Record a successful, profitable trade.
    pub fn record_success(&mut self, profit_mist: i64) {
        self.total_trades += 1;
        self.consecutive_failures = 0;
        self.cumulative_pnl_mist += profit_mist;

        info!(
            profit = %profit_mist,
            cumulative_pnl = %self.cumulative_pnl_mist,
            total_trades = %self.total_trades,
            "Circuit breaker: trade succeeded"
        );
    }

    /// Record a failed or losing trade.
    /// Returns `true` if this trade caused the breaker to trip.
    pub fn record_failure(&mut self, loss_mist: i64, now_ms: u64) -> bool {
        self.total_trades += 1;
        self.consecutive_failures += 1;
        self.cumulative_pnl_mist += loss_mist; // loss_mist should be negative

        warn!(
            consecutive = %self.consecutive_failures,
            loss = %loss_mist,
            cumulative_pnl = %self.cumulative_pnl_mist,
            "Circuit breaker: trade failed/lost"
        );

        // Check trip conditions
        if self.consecutive_failures >= self.max_consecutive_failures {
            self.trip(
                now_ms,
                format!(
                    "{} consecutive failures (limit: {})",
                    self.consecutive_failures, self.max_consecutive_failures
                ),
            );
            return true;
        }

        if self.cumulative_pnl_mist <= -self.max_cumulative_loss_mist {
            self.trip(
                now_ms,
                format!(
                    "Cumulative loss {} MIST exceeds limit {} MIST",
                    self.cumulative_pnl_mist.abs(),
                    self.max_cumulative_loss_mist
                ),
            );
            return true;
        }

        false
    }

    /// Manually trip the breaker.
    fn trip(&mut self, now_ms: u64, reason: String) {
        error!(
            reason = %reason,
            cooldown_ms = %self.cooldown_ms,
            "🚨 CIRCUIT BREAKER TRIPPED — trading paused"
        );
        self.tripped_at_ms = Some(now_ms);
        self.trip_reason = Some(reason);
    }

    /// Reset the breaker state (called after cooldown or manually).
    pub fn reset(&mut self) {
        self.consecutive_failures = 0;
        // Keep cumulative_pnl for accounting, but reset the trip state
        self.tripped_at_ms = None;
        self.trip_reason = None;
    }

    /// Get current stats for logging.
    pub fn stats(&self) -> CircuitBreakerStats {
        CircuitBreakerStats {
            consecutive_failures: self.consecutive_failures,
            cumulative_pnl_mist: self.cumulative_pnl_mist,
            total_trades: self.total_trades,
            is_tripped: self.tripped_at_ms.is_some(),
            trip_reason: self.trip_reason.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CircuitBreakerStats {
    pub consecutive_failures: u32,
    pub cumulative_pnl_mist: i64,
    pub total_trades: u64,
    pub is_tripped: bool,
    pub trip_reason: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_breaker_allows_trading() {
        let mut cb = CircuitBreaker::new(3, 1_000_000, 60_000);
        assert!(cb.is_trading_allowed(0));
    }

    #[test]
    fn test_consecutive_failures_trip() {
        let mut cb = CircuitBreaker::new(3, 1_000_000_000, 60_000);
        assert!(!cb.record_failure(-100_000, 1000)); // 1
        assert!(!cb.record_failure(-100_000, 2000)); // 2
        assert!(cb.record_failure(-100_000, 3000));  // 3 → tripped
        assert!(!cb.is_trading_allowed(3000));
    }

    #[test]
    fn test_success_resets_consecutive_counter() {
        let mut cb = CircuitBreaker::new(3, 1_000_000_000, 60_000);
        cb.record_failure(-100_000, 1000);
        cb.record_failure(-100_000, 2000);
        cb.record_success(500_000); // resets consecutive counter
        assert!(!cb.record_failure(-100_000, 4000)); // only 1 now
        assert!(cb.is_trading_allowed(4000));
    }

    #[test]
    fn test_cumulative_loss_trip() {
        let mut cb = CircuitBreaker::new(100, 500_000, 60_000); // high consec limit
        cb.record_failure(-200_000, 1000);
        cb.record_success(50_000); // resets consecutive but not cumulative
        // cumulative = -200_000 + 50_000 = -150_000
        assert!(cb.is_trading_allowed(2000));
        cb.record_failure(-400_000, 3000);
        // cumulative = -150_000 + -400_000 = -550_000 > 500_000 limit
        assert!(!cb.is_trading_allowed(3000));
    }

    #[test]
    fn test_cooldown_auto_resets() {
        let mut cb = CircuitBreaker::new(1, 1_000_000_000, 5_000); // 5s cooldown
        cb.record_failure(-100_000, 1_000);
        assert!(!cb.is_trading_allowed(2_000)); // too early
        assert!(!cb.is_trading_allowed(5_000)); // still too early
        assert!(cb.is_trading_allowed(6_001));  // cooldown elapsed
    }

    #[test]
    fn test_stats_reporting() {
        let mut cb = CircuitBreaker::new(5, 1_000_000, 60_000);
        cb.record_failure(-100, 1000);
        cb.record_failure(-200, 2000);
        let stats = cb.stats();
        assert_eq!(stats.consecutive_failures, 2);
        assert_eq!(stats.cumulative_pnl_mist, -300);
        assert_eq!(stats.total_trades, 2);
        assert!(!stats.is_tripped);
    }

    #[test]
    fn test_default_config() {
        let cb = CircuitBreaker::default_config();
        assert_eq!(cb.max_consecutive_failures, 5);
        assert_eq!(cb.max_cumulative_loss_mist, 1_000_000_000);
        assert_eq!(cb.cooldown_ms, 60_000);
    }

    #[test]
    fn test_manual_reset() {
        let mut cb = CircuitBreaker::new(1, 1_000_000_000, 60_000);
        cb.record_failure(-100_000, 1000);
        assert!(!cb.is_trading_allowed(1000));
        cb.reset();
        assert!(cb.is_trading_allowed(1000));
    }

    #[test]
    fn test_zero_loss_failures_count() {
        // Even if loss is 0 (e.g., reverted tx with no gas charged), it counts as a failure
        let mut cb = CircuitBreaker::new(2, 1_000_000_000, 60_000);
        cb.record_failure(0, 1000);
        cb.record_failure(0, 2000);
        assert!(!cb.is_trading_allowed(2000));
    }
}
