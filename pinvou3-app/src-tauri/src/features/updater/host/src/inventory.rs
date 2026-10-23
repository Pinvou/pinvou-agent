//! Complete bounded private inventory. Each root/shard is independently parsed
//! under the original metadata limits. The aggregate is never a JSON envelope,
//! native host capability or evidence of an installed namespace.
use crate::{
    error::{Error, Result},
    json,
    profile::{AppFile, RootManifest, Seed, validate_catalog},
};
use serde::{Deserialize, Serialize};
pub(crate) const MAX_SHARDS: usize = 8;
pub(crate) const FILES_PER_SHARD: usize = 4096;
pub(crate) const MAX_FILES: usize = MAX_SHARDS * FILES_PER_SHARD;
pub(crate) const FIRST_RESOURCE_ID: u16 = 34102;
pub(crate) const MAX_SHARD_BYTES: usize = MAX_SHARDS * json::RECORD_MAX;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ShardReference {
    pub(crate) index: u8,
    pub(crate) resource_id: u16,
    pub(crate) size: u64,
    pub(crate) sha256: String,
    pub(crate) file_count: u64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct InventoryDescriptor {
    pub(crate) host_inventory_version: u8,
    pub(crate) file_count: u64,
    pub(crate) total_size: u64,
    pub(crate) shards: Vec<ShardReference>,
}
impl InventoryDescriptor {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.host_inventory_version != 1
            || self.shards.is_empty()
            || self.shards.len() > MAX_SHARDS
            || self.file_count == 0
            || self.file_count > MAX_FILES as u64
            || self.total_size > json::SAFE_MAX
        {
            return Err(Error::ImageRefused);
        }
        let mut count = 0u64;
        let mut bytes = 0u64;
        for (index, reference) in self.shards.iter().enumerate() {
            if reference.index as usize != index
                || reference.resource_id != FIRST_RESOURCE_ID + index as u16
                || reference.size == 0
                || reference.size > json::RECORD_MAX as u64
                || !json::digest(&reference.sha256)
                || reference.file_count == 0
                || reference.file_count > FILES_PER_SHARD as u64
            {
                return Err(Error::ImageRefused);
            }
            count = count
                .checked_add(reference.file_count)
                .ok_or(Error::ImageRefused)?;
            bytes = bytes
                .checked_add(reference.size)
                .ok_or(Error::ImageRefused)?;
        }
        if count != self.file_count || bytes > MAX_SHARD_BYTES as u64 {
            return Err(Error::ImageRefused);
        }
        Ok(())
    }
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InventoryShard {
    host_inventory_shard_version: u8,
    profile_id: String,
    build_id: String,
    seed_sha256: String,
    index: u8,
    files: Vec<AppFile>,
}
/// Exact independently bounded pieces. No constructor for native authority is
/// exposed. Source resources and protected registry values supply these bytes.
pub(crate) struct InventoryBytes {
    pub(crate) root: Vec<u8>,
    pub(crate) shards: Vec<Vec<u8>>,
}
impl InventoryBytes {
    pub(crate) fn parse(&self, seed: &Seed) -> Result<InstallationManifest> {
        let root = RootManifest::parse(&self.root, seed)?;
        if self.shards.len() != root.app_inventory.shards.len() {
            return Err(Error::ImageRefused);
        }
        let mut files = Vec::with_capacity(root.app_inventory.file_count as usize);
        let mut total = 0u64;
        for (bytes, reference) in self.shards.iter().zip(&root.app_inventory.shards) {
            if bytes.len() != reference.size as usize
                || bytes.len() > json::RECORD_MAX
                || json::sha256(bytes) != reference.sha256
            {
                return Err(Error::ImageRefused);
            }
            let shard: InventoryShard =
                json::decode(bytes, false).map_err(|_| Error::ImageRefused)?;
            if shard.host_inventory_shard_version != 1
                || shard.profile_id != root.profile_id
                || shard.build_id != root.build_id
                || shard.seed_sha256 != root.seed_sha256
                || shard.index != reference.index
                || shard.files.len() != reference.file_count as usize
                || json::encode(&shard, false)? != *bytes
            {
                return Err(Error::ImageRefused);
            }
            for file in &shard.files {
                total = total
                    .checked_add(file.size)
                    .filter(|size| *size <= json::SAFE_MAX)
                    .ok_or(Error::ImageRefused)?;
            }
            files.extend(shard.files);
        }
        if total != root.app_inventory.total_size
            || files.len() != root.app_inventory.file_count as usize
        {
            return Err(Error::ImageRefused);
        }
        validate_catalog(&files, &root.app)?;
        Ok(InstallationManifest {
            root,
            app_files: files,
        })
    }
}
/// No Serialize/Deserialize: callers cannot accidentally re-encode the complete
/// aggregate and bypass the per-object parser. Root is the only genesis digest.
pub(crate) struct InstallationManifest {
    pub(crate) root: RootManifest,
    pub(crate) app_files: Vec<AppFile>,
}
impl std::ops::Deref for InstallationManifest {
    type Target = RootManifest;
    fn deref(&self) -> &RootManifest {
        &self.root
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::profile::{ByteReference, ImageReference};
    fn seed() -> Seed {
        serde_json::from_value(serde_json::json!({"hostSeedVersion":1,"profileKind":"lab","profileId":"inventory-model-only","product":"pinvou","component":"agent","targetKey":"windows-x86_64-nsis-perMachine","buildId":"1".repeat(64),"appVersion":"0.0.0","layoutId":"windows-host-v1","hostIpcVersion":1,"hostStateVersion":1,"originalRoot":{"size":1,"sha256":"7".repeat(64)},"approvedInitialPublisherPolicy":{"policyId":"model-only","signerCertificateSha256":"8".repeat(64)}})).unwrap()
    }
    pub(crate) fn fixture(groups: Vec<Vec<AppFile>>) -> (Seed, InventoryBytes) {
        let seed = seed();
        let image = ImageReference {
            build_id: seed.build_id.clone(),
            version: seed.app_version.clone(),
            layout_id: seed.layout_id.clone(),
            size: 1,
            sha256: "2".repeat(64),
        };
        let mut shards = Vec::new();
        let mut references = Vec::new();
        let mut count = 0;
        let mut size = 0;
        for (index, files) in groups.into_iter().enumerate() {
            count += files.len();
            size += files.iter().map(|file| file.size).sum::<u64>();
            let shard = InventoryShard {
                host_inventory_shard_version: 1,
                profile_id: seed.profile_id.clone(),
                build_id: seed.build_id.clone(),
                seed_sha256: seed.seed_sha256(),
                index: index as u8,
                files,
            };
            let bytes = json::encode(&shard, false).unwrap();
            references.push(ShardReference {
                index: index as u8,
                resource_id: FIRST_RESOURCE_ID + index as u16,
                size: bytes.len() as u64,
                sha256: json::sha256(&bytes),
                file_count: shard.files.len() as u64,
            });
            shards.push(bytes);
        }
        let root = RootManifest {
            host_manifest_version: 2,
            profile_kind: seed.profile_kind.clone(),
            profile_id: seed.profile_id.clone(),
            product: seed.product.clone(),
            component: seed.component.clone(),
            target_key: seed.target_key.clone(),
            build_id: seed.build_id.clone(),
            seed_sha256: seed.seed_sha256(),
            original_root: ByteReference {
                size: seed.original_root.size,
                sha256: seed.original_root.sha256.clone(),
            },
            layout_id: seed.layout_id.clone(),
            helper: image.clone(),
            launcher: image.clone(),
            app: image,
            app_inventory: InventoryDescriptor {
                host_inventory_version: 1,
                file_count: count as u64,
                total_size: size,
                shards: references,
            },
        };
        (
            seed,
            InventoryBytes {
                root: json::encode(&root, false).unwrap(),
                shards,
            },
        )
    }
    pub(crate) fn app() -> AppFile {
        AppFile {
            relative_name: "pinvou3-tauri.exe".into(),
            size: 1,
            sha256: "2".repeat(64),
        }
    }
    pub(crate) fn resource(name: &str) -> AppFile {
        AppFile {
            relative_name: name.into(),
            size: 0,
            sha256: "3".repeat(64),
        }
    }
    fn rewrite_root(input: &mut InventoryBytes, change: impl FnOnce(&mut RootManifest)) {
        let mut root: RootManifest = json::decode(&input.root, false).unwrap();
        change(&mut root);
        input.root = json::encode(&root, false).unwrap();
    }
    #[test]
    fn actual_shard_parser_accepts_10877_files_without_an_oversized_input_object() {
        let mut files: Vec<_> = (0..10876)
            .map(|i| resource(&format!("resource-{i:05}.bin")))
            .collect();
        files.push(app());
        files.sort_by(|a, b| a.relative_name.cmp(&b.relative_name));
        let (seed, input) = fixture(
            files
                .chunks(FILES_PER_SHARD)
                .map(|files| files.to_vec())
                .collect(),
        );
        assert!(
            input.shards.len() == 3
                && input
                    .shards
                    .iter()
                    .all(|bytes| bytes.len() <= json::RECORD_MAX)
        );
        assert_eq!(input.parse(&seed).unwrap().app_files.len(), 10877);
        assert!(
            json::encode(&files, false).is_err(),
            "the original single-object array limit stays in force"
        );
    }
    #[test]
    fn shard_binding_corruption_missing_extra_reorder_scope_and_totals_are_refused() {
        let (seed, mut value) = fixture(vec![vec![app()], vec![resource("z")]]);
        assert!(value.parse(&seed).is_ok());
        value.shards.swap(0, 1);
        assert!(value.parse(&seed).is_err());
        value.shards.swap(0, 1);
        assert!(value.parse(&seed).is_ok());
        let last = value.shards.pop().unwrap();
        assert!(value.parse(&seed).is_err());
        value.shards.push(last);
        assert!(value.parse(&seed).is_ok());
        value.shards.push(value.shards[0].clone());
        assert!(value.parse(&seed).is_err());
        value.shards.pop();
        let original = value.shards[0].clone();
        value.shards[0][0] ^= 1;
        assert!(value.parse(&seed).is_err());
        value.shards[0] = original;
        assert!(value.parse(&seed).is_ok());
        for key in ["index", "resource", "count", "total", "version"] {
            let (seed, mut broken) = fixture(vec![vec![app()], vec![resource("z")]]);
            rewrite_root(&mut broken, |root| match key {
                "index" => root.app_inventory.shards[1].index = 0,
                "resource" => root.app_inventory.shards[1].resource_id = 34102,
                "count" => root.app_inventory.file_count += 1,
                "total" => root.app_inventory.total_size += 1,
                _ => root.host_manifest_version = 1,
            });
            assert!(broken.parse(&seed).is_err());
        }
        let mut shard: InventoryShard = json::decode(&value.shards[0], false).unwrap();
        shard.profile_id = "different-profile".into();
        value.shards[0] = json::encode(&shard, false).unwrap();
        let size = value.shards[0].len() as u64;
        let hash = json::sha256(&value.shards[0]);
        rewrite_root(&mut value, |root| {
            root.app_inventory.shards[0].size = size;
            root.app_inventory.shards[0].sha256 = hash;
        });
        assert!(value.parse(&seed).is_err());
    }
    #[test]
    fn eight_shards_are_finite_and_cross_shard_order_and_main_binding_remain_strict() {
        let mut groups = vec![vec![app()]];
        for i in 1..8 {
            groups.push(vec![resource(&format!("z{i}"))]);
        }
        let (seed, input) = fixture(groups);
        assert_eq!(input.parse(&seed).unwrap().app_files.len(), 8);
        let (seed, mut duplicate) = fixture(vec![vec![app()], vec![app()]]);
        assert!(duplicate.parse(&seed).is_err());
        rewrite_root(&mut duplicate, |root| {
            let last = root.app_inventory.shards.last().unwrap().clone();
            root.app_inventory.shards.resize(9, last);
        });
        assert!(duplicate.parse(&seed).is_err());
        let (seed, input) = fixture(vec![vec![resource("z")], vec![app()]]);
        assert!(input.parse(&seed).is_err());
    }
    #[test]
    fn bounded_shards_remain_closed_canonical_objects() {
        let (seed, mut bytes) = fixture(vec![vec![app()], vec![resource("z")]]);
        let original = bytes.shards[0].clone();
        bytes.shards[0].push(b' ');
        let size = bytes.shards[0].len() as u64;
        let hash = json::sha256(&bytes.shards[0]);
        rewrite_root(&mut bytes, |root| {
            root.app_inventory.shards[0].size = size;
            root.app_inventory.shards[0].sha256 = hash;
        });
        assert!(
            bytes.parse(&seed).is_err(),
            "matching hash does not permit noncanonical shard bytes"
        );
        bytes.shards[0] = original;
        let hash = json::sha256(&bytes.shards[0]);
        let size = bytes.shards[0].len() as u64;
        rewrite_root(&mut bytes, |root| {
            root.app_inventory.shards[0].size = size;
            root.app_inventory.shards[0].sha256 = hash;
        });
        assert!(bytes.parse(&seed).is_ok());
        rewrite_root(&mut bytes, |root| {
            root.app_inventory.shards[0].size = json::RECORD_MAX as u64 + 1
        });
        assert!(bytes.parse(&seed).is_err());
    }
}
