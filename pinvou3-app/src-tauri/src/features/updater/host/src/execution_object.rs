//! Unique same-process owner of a physically sealed, signed Package object.
//! Identity/custody is separate from current qualification and install authority.
use crate::{
    download_stage::{BoundPackage, NativeRole, ProtectedByteCapability, poll},
    error::{Error, Result},
    profile::Seed,
};
use pinvou_upgrade_custody::Identity;

/// Immutable facts from the SAME authenticated Package, independent compiled
/// Seed and actual protected seal. No caller facts/JSON constructor is exposed.
#[derive(Clone, PartialEq, Eq)]
pub struct ObjectBinding {
    root_claims_sha256: String,
    package_envelope_sha256: String,
    product: String,
    component: String,
    target_key: String,
    profile_kind: String,
    profile_id: String,
    seed_sha256: String,
    app_version: String,
    package_id: String,
    full_pack_size: u64,
    full_pack_sha256: String,
    role: NativeRole,
    native_claim: Vec<u8>,
    native_size: u64,
    native_sha256: String,
}
impl ObjectBinding {
    pub fn root_claims_sha256(&self) -> &str {
        &self.root_claims_sha256
    }
    pub fn package_envelope_sha256(&self) -> &str {
        &self.package_envelope_sha256
    }
    pub fn product(&self) -> &str {
        &self.product
    }
    pub fn component(&self) -> &str {
        &self.component
    }
    pub fn target_key(&self) -> &str {
        &self.target_key
    }
    pub fn profile_kind(&self) -> &str {
        &self.profile_kind
    }
    pub fn profile_id(&self) -> &str {
        &self.profile_id
    }
    pub fn seed_sha256(&self) -> &str {
        &self.seed_sha256
    }
    pub fn app_version(&self) -> &str {
        &self.app_version
    }
    pub fn package_id(&self) -> &str {
        &self.package_id
    }
    pub fn full_pack_size(&self) -> u64 {
        self.full_pack_size
    }
    pub fn full_pack_sha256(&self) -> &str {
        &self.full_pack_sha256
    }
    pub fn role(&self) -> NativeRole {
        self.role
    }
    /// Canonical five-field signed native declaration, not publisher approval.
    pub fn native_claim(&self) -> &[u8] {
        &self.native_claim
    }
    pub fn native_size(&self) -> u64 {
        self.native_size
    }
    pub fn native_sha256(&self) -> &str {
        &self.native_sha256
    }
}

/// Actual protected FullPack/native file identities, including their original
/// creation fields, plus the COMPLETE binding. No PID, time, path, counter or
/// caller-selected epoch can manufacture this generation. It is not a T28 epoch.
#[derive(Clone, PartialEq, Eq)]
pub struct ObjectGeneration {
    package_identity: Identity,
    native_identity: Identity,
    binding: ObjectBinding,
}
impl ObjectGeneration {
    pub fn package_identity(&self) -> &Identity {
        &self.package_identity
    }
    pub fn native_identity(&self) -> &Identity {
        &self.native_identity
    }
    pub fn binding(&self) -> &ObjectBinding {
        &self.binding
    }
}

/// Cloneable object-identification DATA only. Keeping an old reference does not
/// keep an owner alive, authorize execution, adopt staging or resurrect a process.
/// There is no public constructor or Deserialize/Serialize implementation.
#[derive(Clone, PartialEq, Eq)]
pub struct ObjectReference {
    generation: ObjectGeneration,
}
impl ObjectReference {
    pub fn generation(&self) -> &ObjectGeneration {
        &self.generation
    }
    pub fn binding(&self) -> &ObjectBinding {
        self.generation.binding()
    }
}

/// This factory is called only by the protected staging adapter after the actual
/// FullPack/native copy and role checks. BoundPackage comes from the existing
/// independent compiled-Root signature verifier, not a runtime JSON assertion.
pub(crate) fn reference_from_seal(
    seed: &Seed,
    package: &BoundPackage,
    role: NativeRole,
    package_identity: Identity,
    native_identity: Identity,
) -> Result<ObjectReference> {
    let signed = &package.document.value()["signed"];
    let full = &signed["fullPackage"];
    let native = &signed[role.field()];
    let text =
        |value: &serde_json::Value| value.as_str().map(str::to_owned).ok_or(Error::ImageRefused);
    let size = |value: &serde_json::Value| {
        value
            .as_f64()
            .filter(|n| {
                n.is_finite()
                    && n.fract() == 0.0
                    && (1.0..=crate::json::SAFE_MAX as f64).contains(n)
            })
            .map(|n| n as u64)
            .ok_or(Error::ImageRefused)
    };
    // Preserve the exact signed native identity, not an independently assembled
    // claim or only its hash. All metadata remains tied to the original envelope.
    let native_claim = pinvou_upgrade_codec::Document::parse(
        &serde_json::to_vec(native).map_err(|_| Error::ImageRefused)?,
        pinvou_upgrade_codec::InputKind::Metadata,
    )
    .map_err(|_| Error::ImageRefused)?
    .native_identity_shape()
    .map_err(|_| Error::ImageRefused)?
    .canonical_bytes()
    .to_vec();
    let binding = ObjectBinding {
        root_claims_sha256: package.root_claims_sha256.clone(),
        package_envelope_sha256: package.envelope_sha256.clone(),
        product: seed.product.clone(),
        component: seed.component.clone(),
        target_key: seed.target_key.clone(),
        profile_kind: seed.profile_kind.clone(),
        profile_id: seed.profile_id.clone(),
        seed_sha256: seed.seed_sha256(),
        app_version: text(&signed["appVersion"])?,
        package_id: text(&full["packageId"])?,
        full_pack_size: size(&full["size"])?,
        full_pack_sha256: text(&full["sha256"])?,
        role,
        native_claim,
        native_size: size(&native["size"])?,
        native_sha256: text(&native["sha256"])?,
    };
    Ok(ObjectReference {
        generation: ObjectGeneration {
            package_identity,
            native_identity,
            binding,
        },
    })
}

/// Owns the original protected byte capability. Moving this object moves every
/// original OS lease; no Clone, deserialization, path/handle or launch API exists.
/// This verifies object identity, NOT current Root/key denial, T27 eligibility,
/// T28 preparation/freeze, T32 consume, installation or execution permission.
pub struct VerifiedExecutionObject {
    bytes: ProtectedByteCapability,
    reference: ObjectReference,
}
impl VerifiedExecutionObject {
    pub(crate) fn from_staged(
        mut bytes: ProtectedByteCapability,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<Self> {
        let reference = bytes.seal_reference()?.clone();
        let binding = reference.binding();
        if bytes.role() != binding.role()
            || bytes.size() != binding.native_size()
            || bytes.sha256() != binding.native_sha256()
        {
            return Err(Error::ImageRefused);
        }
        check_transition(&reference, &reference, &mut cancelled, |stop| {
            bytes.recheck(stop)
        })?;
        Ok(Self { bytes, reference })
    }
    pub fn reference(&self) -> &ObjectReference {
        &self.reference
    }

    fn check(
        &mut self,
        expected: &ObjectReference,
        cancelled: &mut dyn FnMut() -> bool,
    ) -> Result<()> {
        if self.bytes.seal_reference()? != &self.reference {
            return Err(Error::ImageRefused);
        }
        check_transition(&self.reference, expected, cancelled, |stop| {
            self.bytes.recheck(stop)
        })
    }

    /// Consume and move this SAME owner only after exact reference/generation/
    /// binding and complete physical revalidation. Failure drops the in-process
    /// owner; it creates no replacement and never adopts a retained directory.
    ///
    /// The old owner cannot be used again, even when the handoff returns an error:
    ///
    /// ```compile_fail
    /// use pinvou_upgrade_host::VerifiedExecutionObject;
    /// fn old_owner_cannot_be_reused(owner: VerifiedExecutionObject) {
    ///     let reference = owner.reference().clone();
    ///     let _next = owner.handoff(&reference, || false);
    ///     let _old_reference = owner.reference(); // owner was moved
    /// }
    /// ```
    pub fn handoff(
        mut self,
        expected: &ObjectReference,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<Self> {
        self.check(expected, &mut cancelled)?;
        Ok(self)
    }

    /// A narrow same-owner borrow after complete physical recheck. Future native
    /// composition must retain this borrow through its own authorized action;
    /// a cloned ObjectReference alone is never an action capability.
    pub fn before_action<'a>(
        &'a mut self,
        expected: &ObjectReference,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<ExecutionObjectGuard<'a>> {
        self.check(expected, &mut cancelled)?;
        Ok(ExecutionObjectGuard { owner: self })
    }
}

/// Keeps the unique owner and every original protected lease continuously alive.
/// It exposes only object facts and bounded reads, no arbitrary action closure.
pub struct ExecutionObjectGuard<'a> {
    owner: &'a mut VerifiedExecutionObject,
}
impl ExecutionObjectGuard<'_> {
    pub fn reference(&self) -> &ObjectReference {
        &self.owner.reference
    }
    pub fn read_at(&mut self, offset: u64, output: &mut [u8]) -> Result<usize> {
        self.owner.bytes.read_at(offset, output)
    }
}

// One transition barrier for initial wrapping, moves and action borrows. The
// callback is private physical revalidation, never a public execution callback.
fn check_transition(
    actual: &ObjectReference,
    expected: &ObjectReference,
    cancelled: &mut dyn FnMut() -> bool,
    recheck: impl FnOnce(&mut dyn FnMut() -> bool) -> Result<()>,
) -> Result<()> {
    let mut stopped = false;
    let mut stop = || {
        stopped |= cancelled();
        stopped
    };
    poll(&mut stop)?;
    if actual != expected {
        return Err(Error::ImageRefused);
    }
    recheck(&mut stop)?;
    poll(&mut stop)
}

#[cfg(test)]
mod tests;
