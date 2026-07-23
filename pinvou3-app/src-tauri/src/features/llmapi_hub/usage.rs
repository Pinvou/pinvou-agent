use chrono::Utc;

use super::models::{current_period, LlmUsageSnapshot};

pub fn record_usage(snapshot: &mut LlmUsageSnapshot, total_tokens: u64) {
    rollover_period_if_needed(snapshot);
    snapshot.used_tokens = snapshot.used_tokens.saturating_add(total_tokens);
    snapshot.remaining_tokens = snapshot.limit_tokens.saturating_sub(snapshot.used_tokens);
    snapshot.last_synced_at = Some(Utc::now());
}

pub fn record_unmetered_call(snapshot: &mut LlmUsageSnapshot) {
    rollover_period_if_needed(snapshot);
    snapshot.unmetered_call_count = snapshot.unmetered_call_count.saturating_add(1);
    snapshot.last_synced_at = Some(Utc::now());
}

pub fn quota_snapshot(snapshot: &LlmUsageSnapshot) -> LlmUsageSnapshot {
    snapshot.clone()
}

fn rollover_period_if_needed(snapshot: &mut LlmUsageSnapshot) {
    let period = current_period();
    if snapshot.period != period {
        snapshot.period = period;
        snapshot.used_tokens = 0;
        snapshot.remaining_tokens = snapshot.limit_tokens;
        snapshot.unmetered_call_count = 0;
        snapshot.last_synced_at = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_usage_and_remaining_tokens() {
        let mut snapshot = LlmUsageSnapshot::new(current_period(), 100);
        record_usage(&mut snapshot, 30);
        assert_eq!(snapshot.used_tokens, 30);
        assert_eq!(snapshot.remaining_tokens, 70);
        assert!(snapshot.last_synced_at.is_some());
    }

    #[test]
    fn records_unmetered_call() {
        let mut snapshot = LlmUsageSnapshot::new(current_period(), 100);
        record_unmetered_call(&mut snapshot);
        assert_eq!(snapshot.unmetered_call_count, 1);
        assert_eq!(snapshot.used_tokens, 0);
    }

    #[test]
    fn saturates_remaining_tokens() {
        let mut snapshot = LlmUsageSnapshot::new(current_period(), 100);
        record_usage(&mut snapshot, 130);
        assert_eq!(snapshot.used_tokens, 130);
        assert_eq!(snapshot.remaining_tokens, 0);
    }
}
