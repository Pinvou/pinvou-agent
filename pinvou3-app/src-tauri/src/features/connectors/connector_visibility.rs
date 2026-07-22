//! Connector skill visibility changes are global, but an Engine turn may be
//! reading the current skill catalogue at the same time. Queue those changes
//! until no submitted turn is active so login/logout affects the next turn.

use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectorKind {
    Eip,
    Zhidao,
}

#[derive(Debug, Default)]
struct VisibilityState {
    active_turns: usize,
    pending_eip: Option<bool>,
    pending_zhidao: Option<bool>,
}

impl VisibilityState {
    fn request(&mut self, kind: ConnectorKind, visible: bool) -> bool {
        if self.active_turns == 0 {
            return false;
        }
        match kind {
            ConnectorKind::Eip => self.pending_eip = Some(visible),
            ConnectorKind::Zhidao => self.pending_zhidao = Some(visible),
        }
        true
    }

    fn finish_turn(&mut self) -> (Option<bool>, Option<bool>) {
        self.active_turns = self.active_turns.saturating_sub(1);
        if self.active_turns == 0 {
            (self.pending_eip.take(), self.pending_zhidao.take())
        } else {
            (None, None)
        }
    }
}

fn state() -> &'static Mutex<VisibilityState> {
    static STATE: OnceLock<Mutex<VisibilityState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(VisibilityState::default()))
}

fn apply(kind: ConnectorKind, visible: bool) {
    let paths = crate::bridge::bundle::Pinvou3Bundle::paths();
    let result = match kind {
        ConnectorKind::Eip => paths.apply_eip_skill_visibility(visible),
        ConnectorKind::Zhidao => paths.apply_zhidao_skill_visibility(visible),
    };
    if let Err(error) = result {
        log::error!("[connector_visibility] failed to apply {kind:?} visible={visible}: {error:#}");
    }
}

pub(crate) fn turn_submitted() {
    let mut guard = state().lock().expect("connector visibility lock poisoned");
    guard.active_turns = guard.active_turns.saturating_add(1);
}

pub(crate) fn turn_finished() {
    let mut guard = state().lock().expect("connector visibility lock poisoned");
    let pending = guard.finish_turn();
    if let Some(visible) = pending.0 {
        apply(ConnectorKind::Eip, visible);
    }
    if let Some(visible) = pending.1 {
        apply(ConnectorKind::Zhidao, visible);
    }
}

/// Apply immediately while idle, otherwise keep only the newest desired state.
pub(crate) fn request(kind: ConnectorKind, visible: bool) -> bool {
    let mut guard = state().lock().expect("connector visibility lock poisoned");
    let deferred = guard.request(kind, visible);
    if !deferred {
        apply(kind, visible);
    }
    deferred
}

#[cfg(test)]
mod tests {
    use super::{ConnectorKind, VisibilityState};

    #[test]
    fn changes_wait_for_the_last_active_turn_and_newest_state_wins() {
        let mut state = VisibilityState {
            active_turns: 2,
            ..VisibilityState::default()
        };
        assert!(state.request(ConnectorKind::Eip, true));
        assert!(state.request(ConnectorKind::Eip, false));
        assert!(state.request(ConnectorKind::Zhidao, true));
        assert_eq!(state.finish_turn(), (None, None));
        assert_eq!(state.finish_turn(), (Some(false), Some(true)));
        assert_eq!(state.finish_turn(), (None, None));
    }
}
