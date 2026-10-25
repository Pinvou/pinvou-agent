//! Transaction-borrowed live time observations from the actual protected owner.
//! No networking, initialization, cleanup completion or caller-DATA source mint.
use super::{
    installed::StartedServiceContext, monotonic::IncarnationCounterSample,
    registry::time::LiveTimeReadSet,
};
use crate::{
    error::{Error, Result},
    trusted_time::{
        Binding, CounterEvidence, Incarnation, UtcInterval, advance_interval, check_root_interval,
    },
};

/// Same-operation evidence only. The actual source borrows held registry key
/// and transaction in LiveTimeReadSet; this object cannot outlive that borrow.
/// No constructor, Clone, Debug, Serialize or generic permission conversion.
/// Its getters are historical DATA at the sampled counter, never a clock that
/// continues to advance merely because somebody retains the object.
pub(super) struct LiveTimeInterval<'read, 'tx> {
    source: &'read LiveTimeReadSet<'tx>,
    current: IncarnationCounterSample,
    bounds: UtcInterval,
}
impl<'read, 'tx> LiveTimeInterval<'read, 'tx> {
    pub(super) fn bounds(&self) -> UtcInterval {
        self.bounds
    }
    /// Only detached actual observation DATA for the private budget record.
    /// A later use must call recheck, not treat these old ticks as current.
    pub(super) fn counter_evidence(&self) -> CounterEvidence {
        CounterEvidence::from_data(self.current.counters())
    }
    /// Re-read the real source and re-sample counters. This returns a NEW
    /// enclosing interval; success does not make the old bounds current.
    pub(super) fn recheck(&self, host: &StartedServiceContext) -> Result<Self> {
        let next = sample_live_time(host, self.source)?;
        next.current.elapsed_since(&self.current)?;
        if next.bounds.earliest_millis() < self.bounds.earliest_millis()
            || next.bounds.latest_millis() < self.bounds.latest_millis()
        {
            return Err(Error::SourceUnavailable);
        }
        Ok(next)
    }
}

/// The private budget publisher calls this with its ACTUAL same-transaction
/// readset while holding its original native mutex/key/transaction. The readset
/// has only registry::time::current_in as its native constructor. Neither a
/// copied State nor caller-provided interval/counter can enter this function.
///
/// cleanup_pending is deliberately not cleared or treated as current rights:
/// this interval supports cleanup-only convergence of existing obligations.
/// A new stage still needs the independent complete-census/barrier gate.
pub(super) fn sample_live_time<'read, 'tx>(
    host: &StartedServiceContext,
    source: &'read LiveTimeReadSet<'tx>,
) -> Result<LiveTimeInterval<'read, 'tx>> {
    source.recheck(host)?;
    let state = source.state();
    if state.binding != Binding::from_host(&host.startup) {
        return Err(Error::ServiceIdentityRefused);
    }
    let incarnation = Incarnation::from_host(&host.startup)?;
    let evidence = state.anchor_for_live(&source.policy().raw, &incarnation)?;
    // Compare actual immutable original bootstrap custody as well as the
    // transaction's accepted current Root. A GET/Seed/parsed Root is no source.
    let original = host.original_root_source()?;
    if original.root_bytes(host)? != source.original_root() {
        return Err(Error::StateCorrupt);
    }
    let current = host.sample_monotonic()?;
    let bounds = advance_interval(
        evidence.earliest,
        evidence.latest,
        evidence.received.data()?,
        current.counters(),
    )?;
    check_root_interval(
        state.current_root(source.original_root()),
        &state.binding.product,
        bounds,
    )?;
    // Fixed same-TX source recheck detects Head/Policy/current host movement
    // during counter acquisition. It cannot clear a barrier or initialize an
    // absent owner. The interval remains at current's actual sample, not the
    // later return/commit instant; the final publisher must freshly recheck.
    source.recheck(host)?;
    // This getter reads the actual global budget watermark in this SAME TX,
    // including this operation's own earlier floor writes. Never clip an old
    // or inconsistent interval with max(); it must independently cover the
    // already protected lower bound or remain unavailable.
    if bounds.earliest_millis() < source.protected_floor_millis()? {
        return Err(Error::SourceUnavailable);
    }
    Ok(LiveTimeInterval {
        source,
        current,
        bounds,
    })
}
