//! A single-purpose in-memory kernel-handle owner. Connection errors, invalid
//! peers and service death leave the same map and listener alive. No install,
//! private-state read, business file write, process launch, kill or name-open.
use super::super::{
    image::SealedImage,
    layout::{Layout, ProtectedDirectory, ordinal_equal},
    pipe::Event,
    process::RetainedProcess,
    registry::Store,
    scm,
};
use super::{
    objects,
    wire::{Action, Binding, Reply, Request},
    *,
};
use crate::{
    json,
    registration::{Phase, PublicRegistration},
};
use std::collections::{BTreeMap, BTreeSet};
use windows_sys::Win32::Foundation::{CloseHandle, CompareObjectHandles};

struct ServicePeer {
    process: RetainedProcess,
    seed: Seed,
    layout: Layout,
    record: PublicRegistration,
    code: ProtectedDirectory,
    helpers: ProtectedDirectory,
    _app_root: ProtectedDirectory,
    helper: SealedImage,
}
impl ServicePeer {
    fn authenticate(pipe: &Pipe) -> Result<Self> {
        let seed = Seed::embedded()?;
        let layout = Layout::discover(&seed)?;
        let record = Store::new(&seed).read_public(&seed)?;
        if record.phase != Phase::Ready {
            return Err(Error::SetupIncomplete);
        }
        let sid = record
            .service_identity
            .service_sid
            .as_deref()
            .ok_or(Error::ServiceIdentityRefused)?;
        let process = scm::live_process(&seed, &layout)?;
        process.verify_host_query_security()?;
        if process.exited()?
            || process.token.user.session_id != 0
            || process.token.user.sid != "S-1-5-18"
            || process.token.network
            || process.token.app_container
            || !process.token.service_sid_enabled
            || pipe.peer_process_id()? != process.selector.process_id
            || pipe.peer_session_id()? != 0
        {
            return Err(Error::ServiceIdentityRefused);
        }
        let identification = pipe.identify_client()?;
        if identification.user != process.token.user
            || identification.integrity != process.token.integrity
            || identification.network
            || identification.app_container
        {
            return Err(Error::ServiceIdentityRefused);
        }
        let (code, app_root) = layout.client_roots(sid)?;
        let helpers = code.child("helpers", false, false, Some(sid))?.child(
            &seed.build_id,
            false,
            false,
            Some(sid),
        )?;
        let helper =
            SealedImage::open(&helpers, "pinvou-upgrade-helper.exe", &record.helper, &seed)?;
        if !ordinal_equal(
            process.image.to_str().ok_or(Error::ImageRefused)?,
            layout
                .helper_path(&seed)
                .to_str()
                .ok_or(Error::ImageRefused)?,
        )? {
            return Err(Error::ImageRefused);
        }
        let result = Self {
            process,
            seed,
            layout,
            record,
            code,
            helpers,
            _app_root: app_root,
            helper,
        };
        result.recheck(pipe)?;
        Ok(result)
    }
    fn recheck(&self, pipe: &Pipe) -> Result<()> {
        if self.process.exited()?
            || pipe.peer_process_id()? != self.process.selector.process_id
            || pipe.peer_session_id()? != 0
            || scm::live_process(&self.seed, &self.layout)?.selector != self.process.selector
            || Store::new(&self.seed).read_public(&self.seed)?.digest()? != self.record.digest()?
        {
            return Err(Error::ServiceIdentityRefused);
        }
        self.code.recheck()?;
        self.helpers.recheck()?;
        if pinvou_upgrade_custody::identity(&self.helper.file).map_err(|_| Error::ImageRefused)?
            != self.helper.identity
            || pinvou_upgrade_custody::identity(&self.helpers.file("pinvou-upgrade-helper.exe")?)
                .map_err(|_| Error::ImageRefused)?
                != self.helper.identity
        {
            return Err(Error::ImageRefused);
        }
        Ok(())
    }
}
struct HeldJob {
    handle: Handle,
    binding: String,
}
struct Actor {
    current: RetainedProcess,
    listener: Pipe,
    stop: Event,
    jobs: BTreeMap<String, HeldJob>,
}
impl Actor {
    fn start() -> Result<Self> {
        let seed = Seed::embedded()?;
        let layout = Layout::discover(&seed)?;
        let record = Store::new(&seed).read_public(&seed)?;
        let sid = record
            .service_identity
            .service_sid
            .as_deref()
            .ok_or(Error::ServiceIdentityRefused)?;
        super::super::keeper_spawn::verify_current_restricted(sid)?;
        let current = RetainedProcess::current()?;
        let service = scm::live_process(&seed, &layout)?;
        if current.selector == service.selector
            || current.token.user != service.token.user
            || current.parent()?.selector != service.selector
            || !ordinal_equal(
                current.image.to_str().ok_or(Error::ImageRefused)?,
                layout
                    .helper_path(&seed)
                    .to_str()
                    .ok_or(Error::ImageRefused)?,
            )?
        {
            return Err(Error::ServiceIdentityRefused);
        }
        // This is the only raw standard-handle adoption. Exact restricted self,
        // parent/SCM and helper identity precede inherited endpoint validation.
        let listener = objects::inherited_listener(&seed)?;
        Ok(Self {
            current,
            listener,
            stop: Event::new()?,
            jobs: BTreeMap::new(),
        })
    }
    fn census(&self, sid: &str) -> Result<Vec<Binding>> {
        let mut result = Vec::with_capacity(self.jobs.len());
        for (name, held) in &self.jobs {
            objects::inspect_job(held.handle.raw(), name, sid)?;
            result.push(Binding {
                job_name: name.clone(),
                job_binding: held.binding.clone(),
                active_processes: objects::query_job(held.handle.raw())?,
            });
        }
        Ok(result)
    }
    #[allow(unsafe_code)]
    fn reconcile_native_jobs(&self, sid: &str) -> Result<bool> {
        let actual = objects::native_jobs(sid)?;
        let mut complete = true;
        for native in &actual {
            if let Some(held) = self.jobs.get(&native.name) {
                if unsafe { CompareObjectHandles(held.handle.raw(), native.raw) } == 0 {
                    return Err(Error::SecurityRefused);
                }
                if held.handle.raw() != native.raw {
                    // Interrupted transfers may leave redundant, unregistered
                    // aliases. Close only after exact native object comparison
                    // to a continuously held mapped query handle. Never erase
                    // an unmatched object or invent its protected binding.
                    drop(Handle::owned(native.raw)?);
                }
            } else {
                complete = false;
            }
        }
        for (name, held) in &self.jobs {
            if !actual
                .iter()
                .any(|n| n.name == *name && n.raw == held.handle.raw())
            {
                return Err(Error::HandoffOutcomeUnknown);
            }
        }
        Ok(complete)
    }
    /// Recover only interrupted typed query transfers for the exact protected
    /// Closed identity selected by this authenticated service's sole Release
    /// sender. That sender owns ClosedWriterRead's operation guard across the
    /// request, forbidding a concurrent direct duplicate/admission.
    ///
    /// The native aliases have continuously kept the original named object
    /// alive. Its unique protected name therefore cannot refer to a replacement
    /// after last-handle closure. Name alone would not justify this operation
    /// for an arbitrary caller or an opened/recreated object; neither is used.
    #[allow(unsafe_code)]
    fn release_unregistered_zero(
        &self,
        service: &ServicePeer,
        job_name: &str,
        sid: &str,
    ) -> Result<()> {
        let actual = objects::native_jobs(sid).map_err(|_| Error::HandoffOutcomeUnknown)?;
        let matching: Vec<_> = actual.iter().filter(|n| n.name == job_name).collect();
        let Some(original) = matching.first() else {
            // Only this keeper's custody is absent. No global Job zero,
            // process-exit, SavedACK or preparation-release inference follows.
            return Ok(());
        };
        let mut addresses = BTreeSet::new();
        // Validate the COMPLETE set before closing ANY handle. Raw native
        // handles remain owned by this process, without a Rust Drop guard that
        // could accidentally close the last alias on an uncertain return.
        for alias in &matching {
            if !addresses.insert(alias.raw as usize)
                || self
                    .jobs
                    .values()
                    .any(|held| held.handle.raw() == alias.raw)
            {
                return Err(Error::HandoffOutcomeUnknown);
            }
            objects::inspect_job(alias.raw, job_name, sid)
                .map_err(|_| Error::HandoffOutcomeUnknown)?;
            if objects::query_job(alias.raw).map_err(|_| Error::HandoffOutcomeUnknown)? != 0
                || unsafe { CompareObjectHandles(original.raw, alias.raw) } == 0
            {
                return Err(Error::HandoffOutcomeUnknown);
            }
        }
        service.recheck(&self.listener)?;
        // The actual original alias stays open throughout every comparison and
        // close below. A partial close/error leaves it (and remaining aliases)
        // alive, preserving the same namespace entry and conservative Unknown.
        for alias in matching.iter().skip(1) {
            objects::inspect_job(alias.raw, job_name, sid)
                .map_err(|_| Error::HandoffOutcomeUnknown)?;
            if objects::query_job(alias.raw).map_err(|_| Error::HandoffOutcomeUnknown)? != 0
                || unsafe { CompareObjectHandles(original.raw, alias.raw) } == 0
                || unsafe { CloseHandle(alias.raw) } == 0
            {
                return Err(Error::HandoffOutcomeUnknown);
            }
        }
        let remaining = objects::native_jobs(sid).map_err(|_| Error::HandoffOutcomeUnknown)?;
        let remaining: Vec<_> = remaining.iter().filter(|n| n.name == job_name).collect();
        if remaining.len() != 1 || remaining[0].raw != original.raw {
            return Err(Error::HandoffOutcomeUnknown);
        }
        objects::inspect_job(original.raw, job_name, sid)
            .map_err(|_| Error::HandoffOutcomeUnknown)?;
        if objects::query_job(original.raw).map_err(|_| Error::HandoffOutcomeUnknown)? != 0 {
            return Err(Error::HandoffOutcomeUnknown);
        }
        service.recheck(&self.listener)?;
        if unsafe { CloseHandle(original.raw) } == 0 {
            return Err(Error::HandoffOutcomeUnknown);
        }
        Ok(())
    }
    #[allow(unsafe_code)]
    fn dispatch(&mut self) -> Result<()> {
        // Identification applies to the LAST message read. Read only a bounded
        // opaque frame before authentication; do not parse/use its handle fields.
        let bytes = self.listener.read_frame(&self.stop, None)?;
        let service = ServicePeer::authenticate(&self.listener)?;
        let request: Request = json::decode(&bytes, true)?;
        request.validate()?;
        if request.registration_digest != service.record.digest()? {
            return Err(Error::ServiceIdentityRefused);
        }
        let sid = service
            .record
            .service_identity
            .service_sid
            .as_deref()
            .ok_or(Error::ServiceIdentityRefused)?;
        let event = request.event_address()?;
        if self.jobs.values().any(|v| v.handle.raw() == event) || self.listener.raw() == event {
            return Err(Error::SecurityRefused);
        }
        // Authenticate the actual dedicated event before changing Job custody;
        // acknowledgement signalling still occurs only AFTER the action.
        // Own immediately after exact native validation. Every subsequent
        // failure, including cleanup/retention/census/SCM/encode/IO failure,
        // drops this SAME validated current event exactly once.
        let challenge: objects::ValidatedChallenge = objects::take_challenge(
            event,
            service.record.host_epoch,
            request.challenge_serial,
            &request.challenge,
        )?;
        service.recheck(&self.listener)?;
        // A later service request may already exist after THIS caller timed
        // out. Native epoch/serial ordering excludes every equal/later event;
        // only this single actor's already completed predecessors can retire.
        objects::cleanup_orphan_challenges(&challenge)?;
        match &request.action {
            Action::Authenticate { .. } => {}
            Action::Ready { .. } => {
                if !self.jobs.is_empty() || !objects::native_jobs(sid)?.is_empty() {
                    return Err(Error::Referenced);
                }
            }
            Action::Retain {
                job_name,
                job_binding,
                handle,
            } => {
                let raw = super::wire::address(handle)?;
                if raw == event
                    || raw == self.listener.raw()
                    || self
                        .jobs
                        .iter()
                        .any(|(name, v)| name != job_name && v.handle.raw() == raw)
                {
                    return Err(Error::SecurityRefused);
                }
                objects::inspect_job(raw, job_name, sid)?;
                // All native type/access/name checks and actual service proof
                // precede adoption. A retained object is never rolled back on
                // a subsequent event/IO/SCM failure.
                if let Some(existing) = self.jobs.get_mut(job_name) {
                    if existing.binding != *job_binding
                        || unsafe { CompareObjectHandles(existing.handle.raw(), raw) } == 0
                    {
                        return Err(Error::SecurityRefused);
                    }
                    // Keep the new exact remote address alive through the
                    // service's post-ACK duplicate-back check. It replaces an
                    // alias of the SAME native object, never another writer.
                    if existing.handle.raw() != raw {
                        let replacement = Handle::owned(raw)?;
                        drop(std::mem::replace(&mut existing.handle, replacement));
                    }
                } else {
                    if self.jobs.len() >= 128 {
                        return Err(Error::Busy);
                    }
                    self.jobs.insert(
                        job_name.clone(),
                        HeldJob {
                            handle: Handle::owned(raw)?,
                            binding: job_binding.clone(),
                        },
                    );
                }
            }
            Action::Census { .. } => {}
            Action::Release {
                job_name,
                job_binding,
            } => {
                if let Some(held) = self.jobs.get(job_name) {
                    if held.binding != *job_binding {
                        return Err(Error::SecurityRefused);
                    }
                    objects::inspect_job(held.handle.raw(), job_name, sid)?;
                    if objects::query_job(held.handle.raw())? != 0 {
                        return Err(Error::Referenced);
                    }
                    // First close native aliases while this exact object stays
                    // held. Unknown native jobs block a closed census.
                    if !self.reconcile_native_jobs(sid)? {
                        return Err(Error::HandoffOutcomeUnknown);
                    }
                    drop(self.jobs.remove(job_name));
                } else {
                    // A service crash may have left exact typed query aliases
                    // before retain delivered, with no map binding or remembered
                    // remote address. The real Closed sender permits only this
                    // native same-object/zero retirement, not a new admission.
                    self.release_unregistered_zero(&service, job_name, sid)?;
                }
                // A repeated release may observe this exact object ABSENT from
                // the complete native keeper handle table. That proves only
                // keeper custody absence, never global job zero, process death,
                // SavedACK or preparation release. Its sole sender holds the
                // actual protected Closed token throughout this request.
            }
        }
        // Connection authentication remains available when jobs or unknown
        // pending transfers exist. It cannot be used as an empty/ready proof.
        let (census, complete) = if matches!(&request.action, Action::Authenticate { .. }) {
            (Vec::new(), false)
        } else {
            (self.census(sid)?, self.reconcile_native_jobs(sid)?)
        };
        service.recheck(&self.listener)?;
        let reply = Reply {
            keeper_wire_version: 1,
            challenge: request.challenge.clone(),
            request_sha256: json::sha256(&json::encode(&request, true)?),
            keeper: self.current.selector.clone(),
            retained: census,
            native_complete: complete,
        };
        // Only the actual trusted service can supply this exact fresh event
        // address, and its native nonce name/access/type are checked here.
        objects::signal_challenge(&challenge)?;
        service.recheck(&self.listener)?;
        self.listener.write_frame(
            &json::encode(&reply, true)?,
            &self.stop,
            Some(service.process.raw()),
        )
    }
}
pub(in crate::platform::windows) fn run() -> Result<()> {
    let mut actor = Actor::start()?;
    // A disconnect, malformed request or new service incarnation never drops
    // the jobs. There is no external stop/exit command. The fixed helper lives
    // until OS termination; paired service+keeper death remains Unknown.
    loop {
        if actor.listener.accept(&actor.stop).is_ok() {
            let _ = actor.dispatch();
        }
        actor.listener.disconnect();
        // Failed native accept can return immediately; keep custody without a
        // busy loop. No timer is treated as authorization or process death.
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}
