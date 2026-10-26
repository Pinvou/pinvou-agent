//! Pure pair validation only; these fixtures cannot mint a native scope or lock.
use super::*;
use crate::time_budget::{Assessment, BudgetWindow, TimeInterval};
use pinvou_upgrade_ownership::live::{
    BudgetAssessment, BudgetObservation, BudgetOrigin, BudgetPresence, Context, ExistingOperation,
    Inventory, LiveOwnership, OtherOccupancy, PreparationIdentity, Purpose,
};

fn binding() -> Binding {
    Binding {
        installation_scope_id: "1".repeat(64),
        product: "pinvou".into(),
        component: "app".into(),
        target_key: "windows-x86_64-nsis-perMachine".into(),
        profile_kind: "production".into(),
        profile_id: "production-test".into(),
        genesis_seed_sha256: "2".repeat(64),
        genesis_manifest_sha256: "3".repeat(64),
        original_install_operation_id: "original:install".into(),
    }
}

fn fixture() -> (OperationRecord, BudgetWindow, String) {
    let native_binding = binding();
    let pure_binding = BootstrapBinding::new(
        &native_binding.installation_scope_id,
        &native_binding.product,
        &native_binding.component,
        &native_binding.target_key,
        &native_binding.profile_kind,
        &native_binding.profile_id,
        &native_binding.genesis_seed_sha256,
    )
    .unwrap();
    let id = WindowId::new(
        native_binding,
        "operation:one".into(),
        WindowKind::PrepFreeze,
    )
    .unwrap();
    let mut window = BudgetWindow::begin_qualified(
        id,
        TimeInterval::new(100, 101).unwrap(),
        120_000,
        Some(100_000),
    )
    .unwrap();
    let Assessment::Before { remaining_ms } = window.assess(Some(window.origin()), None).unwrap()
    else {
        panic!("fixture must have an original remaining budget");
    };
    let key = budgets::window_key(window.id()).unwrap();
    let observation = BudgetObservation {
        origin: BudgetOrigin {
            window_key_sha256: key.clone(),
            first_lower_ms: window.origin().lower_ms(),
            first_upper_ms: window.origin().upper_ms(),
            original_duration_ms: window.original_duration_ms(),
            original_deadline_ms: window.original_deadline_ms(),
            original_session_cutoff_ms: 100_000,
        },
        window_revision: 1,
        spent_max_ms: window.spent_max_ms(),
        session_cutoff_ms: window.session_cutoff_ms().unwrap(),
        assessment: BudgetAssessment::Before { remaining_ms },
    };
    let inventory = Inventory {
        revision: 1,
        sha256: "4".repeat(64),
        participants: Vec::new(),
    };
    let context = Context {
        binding: &pure_binding,
        inventory: &inventory,
        other_occupancy: OtherOccupancy::Clear,
    };
    let state = LiveOwnership::load(
        &ScopeHead::initial(&pure_binding).unwrap(),
        window.id().operation_id(),
        ExistingOperation::NoPrior,
        BudgetPresence::Absent,
    )
    .unwrap()
    .acquire(
        0,
        &context,
        PreparationIdentity {
            operation_id: window.id().operation_id().into(),
            original_session_id: "session:original".into(),
            original_session_sha256: "5".repeat(64),
            request_lineage_sha256: "6".repeat(64),
            purpose: Purpose::Install,
        },
        observation,
    )
    .unwrap();
    (state.write_intent().unwrap().operation_write, window, key)
}

#[test]
fn original_pair_uses_actual_window_identity_and_revision() {
    let (operation, window, key) = fixture();
    assert_eq!(check_window_pair(&operation, &window, 1, &key), Ok(()));
    assert_eq!(
        check_window_pair(&operation, &window, 2, &key),
        Err(Error::StateCorrupt)
    );
    assert_eq!(
        check_window_pair(&operation, &window, 1, &"a".repeat(64)),
        Err(Error::StateCorrupt)
    );
    // The actual window key is a hash, never a raw colon-containing ID path.
    assert!(json::digest(&key));
    assert_ne!(key, operation.operation_id());
}

#[test]
fn partial_budget_publication_cannot_leave_an_apparently_current_owner() {
    let (operation, mut window, key) = fixture();
    window.tighten_session_cutoff(50_000).unwrap();
    assert_eq!(
        check_window_pair(&operation, &window, 1, &key),
        Err(Error::StateCorrupt)
    );
    let (operation, mut window, key) = fixture();
    window
        .assess(Some(TimeInterval::new(200, 201).unwrap()), None)
        .unwrap();
    assert_eq!(
        check_window_pair(&operation, &window, 1, &key),
        Err(Error::StateCorrupt)
    );
}

#[test]
fn another_original_operation_cannot_supply_the_same_budget_pair() {
    let (operation, window, key) = fixture();
    let other = BudgetWindow::begin_qualified(
        WindowId::new(binding(), "operation:other".into(), WindowKind::PrepFreeze).unwrap(),
        window.origin(),
        window.original_duration_ms(),
        window.session_cutoff_ms(),
    )
    .unwrap();
    assert_eq!(
        check_window_pair(&operation, &other, 1, &key),
        Err(Error::StateCorrupt)
    );
}
