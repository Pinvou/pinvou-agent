//! Protected, sharded original product/profile enrollment; not current Root authority.
mod product_root;
use super::*;
use crate::product_catalog::{Event, ProductEvent};
use serde::{Deserialize, Serialize};

fn leaf(seed: &Seed) -> &'static str {
    if seed.profile_kind == "production" {
        "UpgradeProductCatalogV1"
    } else {
        "UpgradeProductCatalogLabV1"
    }
}
fn path(seed: &Seed) -> String {
    format!("Software\\Pinvou\\{}", leaf(seed))
}

/// Lock order: profile BootstrapGuard then this machine guard. Never acquire
/// another profile guard while holding it. All registration writers use it.
pub(super) struct MachineGuard {
    handle: Handle,
    _thread: PhantomData<Rc<()>>,
}
impl MachineGuard {
    #[allow(unsafe_code)]
    pub(super) fn acquire(seed: &Seed) -> Result<Self> {
        use windows_sys::Win32::Security::Authorization::SE_KERNEL_OBJECT;
        let security = NativeSecurity::mutex()?;
        let name = if seed.profile_kind == "production" {
            r"Global\Pinvou.Upgrade.ProductCatalog.v1"
        } else {
            r"Global\Pinvou.Upgrade.ProductCatalog.Lab.v1"
        };
        let handle = Handle::owned(unsafe {
            CreateMutexExW(
                &security.attributes(),
                wide(name).as_ptr(),
                0,
                MUTEX_MODIFY_STATE | SYNCHRONIZE | 0x00020000,
            )
        })?;
        super::super::security::inspect_kernel(handle.raw(), SE_KERNEL_OBJECT)?;
        match unsafe { WaitForSingleObject(handle.raw(), 0) } {
            WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(Self {
                handle,
                _thread: PhantomData,
            }),
            WAIT_TIMEOUT => Err(Error::Busy),
            _ => Err(Error::Unavailable),
        }
    }
}
impl Drop for MachineGuard {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        unsafe {
            ReleaseMutex(self.handle.raw());
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Product {
    id: String,
    root_sha256: String,
    root_size: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProductAnchor {
    product: String,
    revision: u64,
    head_sha256: String,
    original_root_sha256: String,
}
fn anchor_for(product: &Product, head: &ProductEvent, raw: &[u8]) -> ProductAnchor {
    ProductAnchor {
        product: product.id.clone(),
        revision: head.revision,
        head_sha256: json::sha256(raw),
        original_root_sha256: product.root_sha256.clone(),
    }
}
fn check_anchor(key: &Key, descriptor: &Product, head: &ProductEvent, raw: &[u8]) -> Result<()> {
    if read(key, "Record")? != json::encode(&anchor_for(descriptor, head, raw), false)? {
        return Err(Error::StateCorrupt);
    }
    Ok(())
}
impl Product {
    fn of(event: &Event) -> Self {
        Self {
            id: event.product.clone(),
            root_sha256: event.original_root_sha256.clone(),
            root_size: event.original_root_size,
        }
    }
    fn check(&self, key: &Key, name: &str) -> Result<()> {
        if !json::identifier(&self.id)
            || !json::digest(&self.root_sha256)
            || name != json::sha256(self.id.as_bytes())
            || self.root_size == 0
            || self.root_size > json::RECORD_MAX as u64
        {
            return Err(Error::StateCorrupt);
        }
        let root = read(key, "Root")?;
        if root.len() as u64 != self.root_size || json::sha256(&root) != self.root_sha256 {
            return Err(Error::StateCorrupt);
        }
        Ok(())
    }
}

/// Visit one native name at a time. No logical profile-count cap or aggregate
/// catalog buffer is introduced. Failed/overflowed native enumeration is unknown.
#[allow(unsafe_code)]
fn visit(key: &Key, values: bool, mut f: impl FnMut(&str) -> Result<()>) -> Result<()> {
    let mut index = 0u32;
    loop {
        let mut name = [0u16; 129];
        let mut length = name.len() as u32;
        let status = if values {
            unsafe {
                RegEnumValueW(
                    key.0,
                    index,
                    name.as_mut_ptr(),
                    &mut length,
                    ptr::null(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                )
            }
        } else {
            unsafe {
                RegEnumKeyExW(
                    key.0,
                    index,
                    name.as_mut_ptr(),
                    &mut length,
                    ptr::null(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                )
            }
        };
        if status == ERROR_NO_MORE_ITEMS {
            return Ok(());
        }
        if status != 0 || length == 0 || length as usize >= name.len() {
            return Err(Error::Unavailable);
        }
        f(&String::from_utf16(&name[..length as usize]).map_err(|_| Error::StateCorrupt)?)?;
        index = index.checked_add(1).ok_or(Error::Unavailable)?;
    }
}
fn exact(key: &Key, values: bool, expected: &[&str]) -> Result<()> {
    let mut seen = vec![false; expected.len()];
    visit(key, values, |name| {
        let pos = expected
            .iter()
            .position(|item| *item == name)
            .ok_or(Error::StateCorrupt)?;
        if seen[pos] {
            return Err(Error::StateCorrupt);
        }
        seen[pos] = true;
        Ok(())
    })?;
    if seen.iter().all(|item| *item) {
        Ok(())
    } else {
        Err(Error::StateCorrupt)
    }
}
fn entry(parent: &Key, name: &str, tx: Option<&Handle>) -> Result<Key> {
    open(parent.0, name, false, tx)?.ok_or(Error::StateCorrupt)
}
fn event_at(history: &Key, revision: u64, tx: Option<&Handle>) -> Result<(Event, Vec<u8>)> {
    let key = entry(history, &revision_name(revision), tx)?;
    exact(&key, true, &["Record"])?;
    exact(&key, false, &[])?;
    let raw = read(&key, "Record")?;
    let event = Event::parse(&raw)?;
    if event.revision != revision {
        return Err(Error::StateCorrupt);
    }
    Ok((event, raw))
}
fn product_event_at(
    history: &Key,
    revision: u64,
    tx: Option<&Handle>,
) -> Result<(ProductEvent, Vec<u8>)> {
    let key = entry(history, &revision_name(revision), tx)?;
    exact(&key, true, &["Record"])?;
    exact(&key, false, &[])?;
    let raw = read(&key, "Record")?;
    let event = ProductEvent::parse(&raw)?;
    if event.revision != revision {
        return Err(Error::StateCorrupt);
    }
    Ok((event, raw))
}
fn record(key: &Key) -> Result<Product> {
    let raw = read(key, "Record")?;
    let value: Product = json::decode(&raw, false)?;
    if json::encode(&value, false)? != raw {
        return Err(Error::StateCorrupt);
    }
    Ok(value)
}
fn history_name_valid(name: &str, maximum: u64) -> bool {
    name.parse::<u64>().is_ok_and(|revision| {
        revision > 0 && revision <= maximum && revision_name(revision) == name
    })
}
fn validate_product_history(
    product: &Key,
    descriptor: &Product,
    members: &Key,
    global: &Key,
    tx: Option<&Handle>,
    expected: u64,
) -> Result<()> {
    let head_raw = read(product, "Head")?;
    let head = ProductEvent::parse(&head_raw)?;
    if head.revision != expected {
        return Err(Error::StateCorrupt);
    }
    let history = entry(product, "history", tx)?;
    exact(&history, true, &[])?;
    let first_root_binding = product_event_at(&history, 1, tx)?.1;
    product_root::check_in(product, tx, &first_root_binding, &read(product, "Root")?)?;
    let scopes = entry(product, "scopes", tx)?;
    exact(&scopes, true, &[])?;
    let mut previous: Option<(ProductEvent, Vec<u8>)> = None;
    let mut last_global_revision = 0u64;
    for revision in 1..=head.revision {
        let (local, raw) = product_event_at(&history, revision, tx)?;
        if let Some((old, bytes)) = &previous {
            if !local.follows(old, bytes) {
                return Err(Error::StateCorrupt);
            }
        }
        let enrollment = local.enrollment.as_bytes();
        let event = Event::parse(enrollment)?;
        if event.revision <= last_global_revision
            || event.product != descriptor.id
            || event.original_root_sha256 != descriptor.root_sha256
            || event.original_root_size != descriptor.root_size
            || event_at(global, event.revision, tx)?.1 != enrollment
        {
            return Err(Error::StateCorrupt);
        }
        last_global_revision = event.revision;
        check_index(members, &event, enrollment, tx)?;
        let scope = entry(&scopes, &event.scope, tx)?;
        exact(&scope, true, &["Record"])?;
        exact(&scope, false, &[])?;
        if read(&scope, "Record")? != enrollment {
            return Err(Error::StateCorrupt);
        }
        previous = Some((local, raw));
    }
    if previous.as_ref().is_none_or(|(_, raw)| raw != &head_raw) {
        return Err(Error::StateCorrupt);
    }
    let mut count = 0u64;
    visit(&history, false, |name| {
        if !history_name_valid(name, head.revision) {
            return Err(Error::StateCorrupt);
        }
        count = count.checked_add(1).ok_or(Error::Unavailable)?;
        Ok(())
    })?;
    if count != head.revision {
        return Err(Error::StateCorrupt);
    }
    let mut scope_count = 0u64;
    visit(&scopes, false, |name| {
        let scope = entry(&scopes, name, tx)?;
        exact(&scope, true, &["Record"])?;
        exact(&scope, false, &[])?;
        let event = Event::parse(&read(&scope, "Record")?)?;
        if event.product != descriptor.id || name != event.scope {
            return Err(Error::StateCorrupt);
        }
        scope_count = scope_count.checked_add(1).ok_or(Error::Unavailable)?;
        Ok(())
    })?;
    if scope_count != expected {
        return Err(Error::StateCorrupt);
    }
    Ok(())
}
fn check_index(index: &Key, event: &Event, raw: &[u8], tx: Option<&Handle>) -> Result<()> {
    let key = entry(index, &json::sha256(event.profile_id.as_bytes()), tx)?;
    exact(&key, true, &["Record"])?;
    exact(&key, false, &[])?;
    if read(&key, "Record")? != raw {
        return Err(Error::StateCorrupt);
    }
    Ok(())
}

/// Check each registered profile's actual protected initial and current
/// records. Never construct another profile's Seed from this service's Seed.
fn check_profile(seed: &Seed, event: &Event, tx: Option<&Handle>) -> Result<()> {
    let profile_path = format!(
        "Software\\Pinvou\\{}\\{}\\private",
        seed.updater_leaf(),
        event.profile_id
    );
    let private = open(HKEY_LOCAL_MACHINE, &profile_path, false, tx)?.ok_or(Error::StateCorrupt)?;
    let head_raw = read(&private, "Head")?;
    let head: HostRegistration = json::decode(&head_raw, false)?;
    if json::encode(&head, false)? != head_raw
        || head.profile_kind != event.kind
        || head.profile_id != event.profile_id
        || head.product != event.product
        || head.component != event.component
        || head.target_key != event.target_key
        || head.installation_scope_id != event.scope
        || head.genesis_seed_sha256 != event.seed_sha256
        || head.initial_root_reference.sha256 != event.original_root_sha256
        || head.initial_root_reference.size != event.original_root_size
        || head.revision == 0
    {
        return Err(Error::StateCorrupt);
    }
    let original_root = read(&private, METADATA_BOOTSTRAP_ROOT)?;
    if original_root.len() as u64 != event.original_root_size
        || json::sha256(&original_root) != event.original_root_sha256
    {
        return Err(Error::StateCorrupt);
    }
    let history = entry(&private, "history", tx)?;
    let current = entry(&history, &revision_name(head.revision), tx)?;
    if read(&current, "Record")? != head_raw {
        return Err(Error::StateCorrupt);
    }
    let first = entry(&history, &revision_name(1), tx)?;
    let initial_raw = read(&first, "Record")?;
    let initial: HostRegistration = json::decode(&initial_raw, false)?;
    if json::encode(&initial, false)? != initial_raw
        || initial.revision != 1
        || initial.profile_kind != event.kind
        || initial.profile_id != event.profile_id
        || initial.product != event.product
        || initial.component != event.component
        || initial.target_key != event.target_key
        || initial.installation_scope_id != event.scope
        || initial.genesis_seed_sha256 != event.seed_sha256
        || initial.initial_root_reference != head.initial_root_reference
        || initial.genesis_manifest_sha256 != head.genesis_manifest_sha256
        || initial.original_install_operation_id != head.original_install_operation_id
    {
        return Err(Error::StateCorrupt);
    }
    if tx.is_none() && read(&private, "Head")? != head_raw {
        return Err(Error::ScopeConflict);
    }
    Ok(())
}

fn validate(key: &Key, seed: &Seed, tx: Option<&Handle>, head_raw: &[u8]) -> Result<Event> {
    exact(key, true, &["Head"])?;
    exact(
        key,
        false,
        &["history", "profiles", "products", "scopes", "anchors"],
    )?;
    let head = Event::parse(head_raw)?;
    if head.kind != seed.profile_kind {
        return Err(Error::StateCorrupt);
    }
    let history = entry(key, "history", tx)?;
    exact(&history, true, &[])?;
    let profiles = entry(key, "profiles", tx)?;
    let products = entry(key, "products", tx)?;
    let scopes = entry(key, "scopes", tx)?;
    let anchors = entry(key, "anchors", tx)?;
    for k in [&history, &profiles, &products, &scopes, &anchors] {
        exact(k, true, &[])?;
    }
    let mut previous: Option<(Event, Vec<u8>)> = None;
    for revision in 1..=head.revision {
        let (event, raw) = event_at(&history, revision, tx)?;
        if event.kind != seed.profile_kind {
            return Err(Error::StateCorrupt);
        }
        if let Some((old, old_raw)) = &previous {
            event.follows(old, old_raw)?;
        }
        check_index(&profiles, &event, &raw, tx)?;
        let scope = entry(&scopes, &event.scope, tx)?;
        exact(&scope, true, &["Record"])?;
        exact(&scope, false, &[])?;
        if read(&scope, "Record")? != raw {
            return Err(Error::StateCorrupt);
        }
        check_profile(seed, &event, tx)?;
        let product = entry(&products, &json::sha256(event.product.as_bytes()), tx)?;
        let expected = Product::of(&event);
        if read(&product, "Record")? != json::encode(&expected, false)? {
            return Err(Error::StateCorrupt);
        }
        let members = entry(&product, "members", tx)?;
        check_index(&members, &event, &raw, tx)?;
        previous = Some((event, raw));
    }
    if previous.as_ref().is_none_or(|(_, raw)| raw != head_raw) {
        return Err(Error::StateCorrupt);
    }
    let mut history_count = 0u64;
    visit(&history, false, |name| {
        if !history_name_valid(name, head.revision) {
            return Err(Error::StateCorrupt);
        }
        history_count = history_count.checked_add(1).ok_or(Error::Unavailable)?;
        Ok(())
    })?;
    let mut profile_count = 0u64;
    visit(&profiles, false, |name| {
        let member = entry(&profiles, name, tx)?;
        exact(&member, true, &["Record"])?;
        exact(&member, false, &[])?;
        let raw = read(&member, "Record")?;
        let event = Event::parse(&raw)?;
        if name != json::sha256(event.profile_id.as_bytes())
            || event.revision > head.revision
            || event_at(&history, event.revision, tx)?.1 != raw
        {
            return Err(Error::StateCorrupt);
        }
        profile_count = profile_count.checked_add(1).ok_or(Error::Unavailable)?;
        Ok(())
    })?;
    let mut scope_count = 0u64;
    visit(&scopes, false, |name| {
        let scope = entry(&scopes, name, tx)?;
        exact(&scope, true, &["Record"])?;
        exact(&scope, false, &[])?;
        let raw = read(&scope, "Record")?;
        let event = Event::parse(&raw)?;
        if name != event.scope
            || event.revision > head.revision
            || event_at(&history, event.revision, tx)?.1 != raw
        {
            return Err(Error::StateCorrupt);
        }
        scope_count = scope_count.checked_add(1).ok_or(Error::Unavailable)?;
        Ok(())
    })?;
    let mut member_count = 0u64;
    visit(&products, false, |name| {
        let product = entry(&products, name, tx)?;
        exact(&product, true, &["Record", "Root", "Head"])?;
        exact(
            &product,
            false,
            &["members", "history", "scopes", "root-state-v1"],
        )?;
        let descriptor = record(&product)?;
        descriptor.check(&product, name)?;
        let anchor = entry(&anchors, name, tx)?;
        exact(&anchor, true, &["Record"])?;
        exact(&anchor, false, &[])?;
        let product_head_raw = read(&product, "Head")?;
        let product_head = ProductEvent::parse(&product_head_raw)?;
        check_anchor(&anchor, &descriptor, &product_head, &product_head_raw)?;
        let members = entry(&product, "members", tx)?;
        exact(&members, true, &[])?;
        let mut local = 0u64;
        visit(&members, false, |member_name| {
            let member = entry(&members, member_name, tx)?;
            exact(&member, true, &["Record"])?;
            exact(&member, false, &[])?;
            let raw = read(&member, "Record")?;
            let event = Event::parse(&raw)?;
            if event.product != descriptor.id
                || event.original_root_sha256 != descriptor.root_sha256
                || event.original_root_size != descriptor.root_size
                || member_name != json::sha256(event.profile_id.as_bytes())
                || event.revision > head.revision
                || event_at(&history, event.revision, tx)?.1 != raw
            {
                return Err(Error::StateCorrupt);
            }
            check_index(&profiles, &event, &raw, tx)?;
            local = local.checked_add(1).ok_or(Error::Unavailable)?;
            member_count = member_count.checked_add(1).ok_or(Error::Unavailable)?;
            Ok(())
        })?;
        if local == 0 {
            return Err(Error::StateCorrupt);
        }
        validate_product_history(&product, &descriptor, &members, &history, tx, local)?;
        Ok(())
    })?;
    let mut anchor_count = 0u64;
    visit(&anchors, false, |name| {
        let anchor = entry(&anchors, name, tx)?;
        exact(&anchor, true, &["Record"])?;
        exact(&anchor, false, &[])?;
        let product = entry(&products, name, tx)?;
        let descriptor = record(&product)?;
        let raw = read(&product, "Head")?;
        check_anchor(&anchor, &descriptor, &ProductEvent::parse(&raw)?, &raw)?;
        anchor_count = anchor_count.checked_add(1).ok_or(Error::Unavailable)?;
        Ok(())
    })?;
    let mut product_count = 0u64;
    visit(&products, false, |_| {
        product_count = product_count.checked_add(1).ok_or(Error::Unavailable)?;
        Ok(())
    })?;
    if anchor_count != product_count {
        return Err(Error::StateCorrupt);
    }
    if history_count != head.revision
        || profile_count != head.revision
        || member_count != head.revision
        || scope_count != head.revision
    {
        return Err(Error::StateCorrupt);
    }
    Ok(head)
}

fn load(seed: &Seed, tx: Option<&Handle>) -> Result<Option<(Event, Vec<u8>)>> {
    let Some(key) = open(HKEY_LOCAL_MACHINE, &path(seed), false, tx)? else {
        return Ok(None);
    };
    let raw = read(&key, "Head")?;
    let checked = validate(&key, seed, tx, &raw);
    if tx.is_none() && read(&key, "Head")? != raw {
        return Err(Error::ScopeConflict);
    }
    checked.map(|head| Some((head, raw)))
}
fn require_no_profiles(seed: &Seed, tx: &Handle) -> Result<()> {
    let profiles = format!("Software\\Pinvou\\{}", seed.updater_leaf());
    if let Some(key) = open(HKEY_LOCAL_MACHINE, &profiles, false, Some(tx))? {
        visit(&key, true, |_| Err(Error::SetupIncomplete))?;
        visit(&key, false, |_| Err(Error::SetupIncomplete))?;
    }
    Ok(())
}

pub(super) fn stage(seed: &Seed, host: &HostRegistration, tx: &Handle) -> Result<()> {
    let old = load(seed, Some(tx))?;
    if old.is_none() {
        require_no_profiles(seed, tx)?;
    }
    let next = Event::enroll(old.as_ref().map(|(e, raw)| (e, raw.as_slice())), seed, host)?;
    let raw = json::encode(&next, false)?;
    let software =
        open(HKEY_LOCAL_MACHINE, "Software", true, Some(tx))?.ok_or(Error::Unavailable)?;
    let pinvou = create(software.0, "Pinvou", tx, true, false)?;
    let key = create(pinvou.0, leaf(seed), tx, false, old.is_none())?;
    let history = create(key.0, "history", tx, false, old.is_none())?;
    let profiles = create(key.0, "profiles", tx, false, old.is_none())?;
    let products = create(key.0, "products", tx, false, old.is_none())?;
    let scopes = create(key.0, "scopes", tx, false, old.is_none())?;
    let anchors = create(key.0, "anchors", tx, false, old.is_none())?;
    let profile_hash = json::sha256(next.profile_id.as_bytes());
    let product_hash = json::sha256(next.product.as_bytes());
    let profile = create(profiles.0, &profile_hash, tx, false, true)?;
    let scope = create(scopes.0, &next.scope, tx, false, true)?;
    let existing = open(products.0, &product_hash, true, Some(tx))?;
    let new_product = existing.is_none();
    let product = match existing {
        Some(product) => {
            if read(&product, "Record")? != json::encode(&Product::of(&next), false)?
                || read(&product, "Root")? != seed.original_root_bytes()
            {
                return Err(Error::ScopeConflict);
            }
            product
        }
        None => {
            let product = create(products.0, &product_hash, tx, false, true)?;
            write(
                &product,
                "Record",
                &json::encode(&Product::of(&next), false)?,
            )?;
            write(&product, "Root", seed.original_root_bytes())?;
            product
        }
    };
    let members = create(product.0, "members", tx, false, false)?;
    let product_scopes = create(product.0, "scopes", tx, false, new_product)?;
    let product_scope = create(product_scopes.0, &next.scope, tx, false, true)?;
    let product_history = create(product.0, "history", tx, false, new_product)?;
    let old_product = if new_product {
        None
    } else {
        let old_raw = read(&product, "Head")?;
        Some((ProductEvent::parse(&old_raw)?, old_raw))
    };
    let product_event = ProductEvent::next(
        old_product
            .as_ref()
            .map(|(event, bytes)| (event, bytes.as_slice())),
        &raw,
    )?;
    let product_raw = json::encode(&product_event, false)?;
    if new_product {
        product_root::initialize_in(&product, tx, &product_raw, seed.original_root_bytes())?;
    }
    let anchor = create(anchors.0, &product_hash, tx, false, new_product)?;
    let product_revision = create(
        product_history.0,
        &revision_name(product_event.revision),
        tx,
        false,
        true,
    )?;
    let member = create(members.0, &profile_hash, tx, false, true)?;
    let history_entry = create(history.0, &revision_name(next.revision), tx, false, true)?;
    write(&profile, "Record", &raw)?;
    write(&scope, "Record", &raw)?;
    write(&member, "Record", &raw)?;
    write(&product_scope, "Record", &raw)?;
    write(&history_entry, "Record", &raw)?;
    write(&product_revision, "Record", &product_raw)?;
    write(&product, "Head", &product_raw)?;
    write(
        &anchor,
        "Record",
        &json::encode(
            &anchor_for(&Product::of(&next), &product_event, &product_raw),
            false,
        )?,
    )?;
    write(&key, "Head", &raw)?;
    Ok(())
}
pub(super) fn check(seed: &Seed, host: &HostRegistration, tx: Option<&Handle>) -> Result<()> {
    let key = open(HKEY_LOCAL_MACHINE, &path(seed), false, tx)?.ok_or(Error::SetupIncomplete)?;
    let products = entry(&key, "products", tx)?;
    let product_name = json::sha256(seed.product.as_bytes());
    let product = entry(&products, &product_name, tx)?;
    let anchors = entry(&key, "anchors", tx)?;
    let anchor = entry(&anchors, &product_name, tx)?;
    exact(&anchor, true, &["Record"])?;
    exact(&anchor, false, &[])?;
    let anchor_raw = read(&anchor, "Record")?;
    let head_raw = read(&product, "Head")?;
    let checked = check_product(&product, &product_name, seed, host, tx, &head_raw);
    if tx.is_none()
        && (read(&product, "Head")? != head_raw || read(&anchor, "Record")? != anchor_raw)
    {
        return Err(Error::ScopeConflict);
    }
    checked?;
    let expected = json::encode(
        &anchor_for(
            &record(&product)?,
            &ProductEvent::parse(&head_raw)?,
            &head_raw,
        ),
        false,
    )?;
    if anchor_raw != expected {
        return Err(Error::StateCorrupt);
    }
    Ok(())
}

// Ordinary reads use only this product's protected history and members.
fn check_product(
    product: &Key,
    product_name: &str,
    seed: &Seed,
    host: &HostRegistration,
    tx: Option<&Handle>,
    head_raw: &[u8],
) -> Result<()> {
    exact(product, true, &["Record", "Root", "Head"])?;
    exact(
        product,
        false,
        &["members", "history", "scopes", "root-state-v1"],
    )?;
    let descriptor = record(product)?;
    descriptor.check(product, product_name)?;
    if descriptor.id != seed.product {
        return Err(Error::StateCorrupt);
    }
    let head = ProductEvent::parse(head_raw)?;
    let history = entry(product, "history", tx)?;
    exact(&history, true, &[])?;
    let first_root_binding = product_event_at(&history, 1, tx)?.1;
    product_root::check_in(product, tx, &first_root_binding, &read(product, "Root")?)?;
    let members = entry(product, "members", tx)?;
    exact(&members, true, &[])?;
    let scopes = entry(product, "scopes", tx)?;
    exact(&scopes, true, &[])?;
    let mut previous: Option<(ProductEvent, Vec<u8>)> = None;
    let mut last_global_revision = 0u64;
    let mut found = false;
    for revision in 1..=head.revision {
        let (local, raw) = product_event_at(&history, revision, tx)?;
        if let Some((old, bytes)) = &previous {
            if !local.follows(old, bytes) {
                return Err(Error::StateCorrupt);
            }
        }
        let enrollment = local.enrollment.as_bytes();
        let event = Event::parse(enrollment)?;
        if event.revision <= last_global_revision
            || event.kind != seed.profile_kind
            || event.product != seed.product
            || event.original_root_sha256 != descriptor.root_sha256
            || event.original_root_size != descriptor.root_size
        {
            return Err(Error::StateCorrupt);
        }
        last_global_revision = event.revision;
        check_index(&members, &event, enrollment, tx)?;
        let scope = entry(&scopes, &event.scope, tx)?;
        exact(&scope, true, &["Record"])?;
        exact(&scope, false, &[])?;
        if read(&scope, "Record")? != enrollment {
            return Err(Error::StateCorrupt);
        }
        check_profile(seed, &event, tx)?;
        if event.matches(seed, host) {
            if found {
                return Err(Error::StateCorrupt);
            }
            found = true;
        }
        previous = Some((local, raw));
    }
    if previous.as_ref().is_none_or(|(_, raw)| raw != head_raw) || !found {
        return Err(Error::StateCorrupt);
    }
    let mut history_count = 0u64;
    visit(&history, false, |name| {
        if !history_name_valid(name, head.revision) {
            return Err(Error::StateCorrupt);
        }
        history_count = history_count.checked_add(1).ok_or(Error::Unavailable)?;
        Ok(())
    })?;
    if history_count != head.revision {
        return Err(Error::StateCorrupt);
    }
    let mut count = 0u64;
    visit(&members, false, |name| {
        let member = entry(&members, name, tx)?;
        exact(&member, true, &["Record"])?;
        exact(&member, false, &[])?;
        let raw = read(&member, "Record")?;
        let event = Event::parse(&raw)?;
        if event.product != seed.product || name != json::sha256(event.profile_id.as_bytes()) {
            return Err(Error::StateCorrupt);
        }
        count = count.checked_add(1).ok_or(Error::Unavailable)?;
        Ok(())
    })?;
    if count != head.revision {
        return Err(Error::StateCorrupt);
    }
    let mut scope_count = 0u64;
    visit(&scopes, false, |name| {
        let scope = entry(&scopes, name, tx)?;
        exact(&scope, true, &["Record"])?;
        exact(&scope, false, &[])?;
        let event = Event::parse(&read(&scope, "Record")?)?;
        if name != event.scope || event.product != seed.product {
            return Err(Error::StateCorrupt);
        }
        scope_count = scope_count.checked_add(1).ok_or(Error::Unavailable)?;
        Ok(())
    })?;
    if scope_count != head.revision {
        return Err(Error::StateCorrupt);
    }
    Ok(())
}
#[allow(unsafe_code)]
pub(super) fn verify_committed(seed: &Seed, host: &HostRegistration) -> Result<()> {
    let key =
        open(HKEY_LOCAL_MACHINE, &path(seed), false, None)?.ok_or(Error::StateCommitUnknown)?;
    let history = entry(&key, "history", None)?;
    let profiles = entry(&key, "profiles", None)?;
    let products = entry(&key, "products", None)?;
    let scopes = entry(&key, "scopes", None)?;
    let anchors = entry(&key, "anchors", None)?;
    let anchor = entry(&anchors, &json::sha256(seed.product.as_bytes()), None)?;
    let profile = entry(&profiles, &json::sha256(seed.profile_id.as_bytes()), None)?;
    let event = Event::parse(&read(&profile, "Record")?)?;
    let scope = entry(&scopes, &event.scope, None)?;
    let revision = entry(&history, &revision_name(event.revision), None)?;
    let product = entry(&products, &json::sha256(seed.product.as_bytes()), None)?;
    product_root::flush(&product)?;
    let members = entry(&product, "members", None)?;
    let product_scopes = entry(&product, "scopes", None)?;
    let product_scope = entry(&product_scopes, &event.scope, None)?;
    let product_history = entry(&product, "history", None)?;
    let product_head = ProductEvent::parse(&read(&product, "Head")?)?;
    let product_revision = entry(
        &product_history,
        &revision_name(product_head.revision),
        None,
    )?;
    let member = entry(&members, &json::sha256(seed.profile_id.as_bytes()), None)?;
    for item in [
        &key,
        &history,
        &profiles,
        &products,
        &scopes,
        &profile,
        &scope,
        &revision,
        &product,
        &members,
        &member,
        &product_scopes,
        &product_scope,
        &product_history,
        &product_revision,
        &anchors,
        &anchor,
    ] {
        if unsafe { RegFlushKey(item.0) } != 0 {
            return Err(Error::StateCommitUnknown);
        }
    }
    check(seed, host, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn history_names_use_exact_decimal_revision() {
        assert!(history_name_valid("0000000000000010", 10));
        assert!(history_name_valid("0000000000000016", 16));
        assert!(!history_name_valid("0000000000000010", 9));
        assert!(!history_name_valid("0000000000000000", 10));
        assert!(!history_name_valid("10", 10));
        assert!(!history_name_valid("000000000000000a", 10));
    }
}
