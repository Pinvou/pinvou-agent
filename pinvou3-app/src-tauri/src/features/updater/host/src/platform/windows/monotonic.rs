//! Real Win32 interrupt counters and SCM-incarnation-bound sampling.
//! No UTC, persistent state, Root source, policy default or execution permission.
use super::{installed::StartedServiceContext, registry::Store};
use crate::{
    error::{Error, Result},
    identity::ProcessSelector,
    monotonic::CounterObservation,
};
use windows_sys::Win32::System::WindowsProgramming::{
    QueryInterruptTimePrecise, QueryUnbiasedInterruptTimePrecise,
};

/// Raw native DATA only; a normal process can call these APIs. Read ordering
/// encloses the two awake samples within the suspend-inclusive clock window.
#[allow(unsafe_code)]
pub(super) fn read_counters() -> Result<CounterObservation> {
    let mut interrupt_before = u64::MAX;
    let mut awake_before = u64::MAX;
    let mut awake_after = u64::MAX;
    let mut interrupt_after = u64::MAX;
    // The pinned windows-sys bindings take aligned *mut u64 and return VOID.
    // These APIs do not use wall time; both counters have 100ns units.
    unsafe {
        QueryInterruptTimePrecise(&mut interrupt_before);
        QueryUnbiasedInterruptTimePrecise(&mut awake_before);
        QueryUnbiasedInterruptTimePrecise(&mut awake_after);
        QueryInterruptTimePrecise(&mut interrupt_after);
    }
    if [interrupt_before, awake_before, awake_after, interrupt_after].contains(&u64::MAX) {
        return Err(Error::Unavailable);
    }
    CounterObservation::from_bracket(interrupt_before, awake_before, awake_after, interrupt_after)
}

#[derive(PartialEq, Eq)]
struct IncarnationBinding {
    installation_scope_id: String,
    host_epoch: u64,
    instance_nonce: String,
    process: ProcessSelector,
}

/// Source sample minted only by a real post-COMMIT StartedServiceContext below.
/// Private immutable fields; no public constructor, Clone, Debug or Serialize.
/// This attests the sampled incarnation only, not continued liveness or UTC.
pub(super) struct IncarnationCounterSample {
    binding: IncarnationBinding,
    counters: CounterObservation,
}
impl IncarnationCounterSample {
    pub(super) fn counters(&self) -> CounterObservation {
        self.counters
    }
    pub(super) fn same_incarnation(&self, other: &Self) -> bool {
        self.binding == other.binding
    }
    pub(super) fn elapsed_since(&self, previous: &Self) -> Result<CounterObservation> {
        if !self.same_incarnation(previous) {
            return Err(Error::ServiceIdentityRefused);
        }
        self.counters.elapsed_since(previous.counters)
    }
}

impl StartedServiceContext {
    /// This is intentionally absent from InstalledServiceContext: its startup
    /// incarnation has not yet committed. No ordinary-process/test source port.
    /// Every read authenticates the real running SCM/PID, retained image and
    /// protected registration before AND after the native counter calls.
    /// Counter windows describe the actual calls, not the later return instant:
    /// callers must account for subsequent authentication/processing elapsed
    /// time with further same-incarnation observations, never reset an RTT origin.
    pub(super) fn sample_monotonic(&self) -> Result<IncarnationCounterSample> {
        let own = &self.context;
        let store = Store::new(&own.seed);
        let before = store.read(&own.seed)?.ok_or(Error::StateCorrupt)?;
        self.authenticate_running(&before)?;
        let counters = read_counters()?;
        let after = store.read(&own.seed)?.ok_or(Error::StateCorrupt)?;
        self.authenticate_running(&after)?;
        let startup = after
            .startup_attestation
            .as_ref()
            .ok_or(Error::SetupIncomplete)?;
        // authenticate_running already compares actual native process identity,
        // scope, epoch and instance nonce with this exact committed startup.
        let binding = IncarnationBinding {
            installation_scope_id: after.installation_scope_id.clone(),
            host_epoch: after.host_epoch,
            instance_nonce: startup.instance_nonce.clone(),
            process: own.process.selector.clone(),
        };
        Ok(IncarnationCounterSample { binding, counters })
    }
}
