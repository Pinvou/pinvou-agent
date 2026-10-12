//! mcp.json 旧版明文密钥迁移:把早期写进 manifest/mcp.json 的明文 API Key 搬到
//! 系统凭据库,文件里只留 `${ENV}` 占位符。
// architecture-guard: allow-target-cfg -- the round-27 in-lock mcp.json FIFO
// pin is cfg(unix)-gated: plant_fifo and the O_NONBLOCK refusal it exercises
// are unix-only; the bounded worker fails a raw-read regression loudly
// instead of hanging the lane on the held mcp.lock (test-only, same
// exemption precedent as marketplace/store.rs).

use crate::platform::paths;

use super::MarketplaceManager;
use super::connectors::{with_mcp_json_lock, write_json_pretty};
use super::secrets::{
    mcp_secret_env_var, mcp_secret_placeholder, mcp_secret_reference, mcp_secret_store_error,
    store_secret_value,
};

/// Reads a migration source through the hardened read primitive; NotFound is a no-op for migration sources.
fn read_present_private_data_file(path: &std::path::Path) -> Result<Option<String>, String> {
    match crate::platform::filesystem::read_private_data_file(path) {
        Ok(content) => Ok(Some(content)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("读取 {} 失败: {e}", path.display())),
    }
}

/// 内置的"已知有明文密钥的工具"清单(迁移目标)。
#[derive(Debug, Clone, Copy)]
struct LegacyMcpSecretSpec {
    tool_id: &'static str,
    target: &'static str,
    key: &'static str,
}

fn legacy_mcp_secret_specs() -> &'static [LegacyMcpSecretSpec] {
    &[
        LegacyMcpSecretSpec {
            tool_id: "weather",
            target: "env",
            key: "AMAP_KEY",
        },
        LegacyMcpSecretSpec {
            tool_id: "iwencai",
            target: "env",
            key: "IWENCAI_API_KEY",
        },
        LegacyMcpSecretSpec {
            tool_id: "qcc",
            target: "header",
            key: "QCC_API_KEY",
        },
    ]
}

fn legacy_spec_for_server_name(server_name: &str) -> Option<&'static LegacyMcpSecretSpec> {
    legacy_mcp_secret_specs()
        .iter()
        .find(|spec| match spec.tool_id {
            "qcc" => server_name.starts_with("qcc-"),
            tool_id => server_name == tool_id,
        })
}

impl<S: crate::platform::credential_store::CredentialStore> MarketplaceManager<S> {
    /// Migrates historical plaintext secrets into the system credential store. Per-entry results only go to the log (previously aggregated into
    /// a returned `McpSecretMigrationResult`, but every caller only consumed success/failure and the details were never
    /// read); any single migration failure aborts with Err, leaving rollback/skip to the caller.
    pub fn migrate_mcp_plaintext_secrets(&self) -> Result<(), String> {
        for spec in legacy_mcp_secret_specs() {
            let path = self.servers_dir.join(spec.tool_id).join("manifest.json");
            if path.is_file() {
                self.migrate_manifest_file(&path, spec)?;
            }
        }
        let mcp_path = paths::mcp_config_path();
        if mcp_path.is_file() {
            self.migrate_mcp_json_file(&mcp_path)?;
        }
        Ok(())
    }

    fn migrate_manifest_file(
        &self,
        path: &std::path::Path,
        spec: &LegacyMcpSecretSpec,
    ) -> Result<(), String> {
        // 同一族裸读——本读可达时调用方持有
        // MARKETPLACE_TRANSACTION_LOCK(install_inner),probe→open 的 swap 窗口
        // 内被换成 FIFO 的 manifest 会楔死该进程的安装道。与 mcp.json 段同一
        // 加固原语;NotFound(被并发删走)按缺失条目无操作处理。
        let Some(content) = read_present_private_data_file(path)? else {
            return Ok(());
        };
        let mut json: serde_json::Value = serde_json::from_str(&content)
            .map_err(|e| format!("解析 {} 失败: {e}", path.display()))?;
        let Some(value) = json
            .get("env")
            .and_then(|env| env.get(spec.key))
            .and_then(|v| v.as_str())
            // 与 mcp.json 段同口径：占位符文本不是明文——凭据库被清空时把它
            // 当 secret 存回去，等于把占位符伪装成凭据。
            .filter(|v| !v.trim().is_empty() && v.trim() != mcp_secret_placeholder(spec.key))
            .map(ToOwned::to_owned)
        else {
            return Ok(());
        };

        self.store_migrated_secret(spec, &value)?;
        if let Some(env) = json.get_mut("env").and_then(|env| env.as_object_mut()) {
            env.remove(spec.key);
            if env.is_empty() {
                json.as_object_mut().map(|obj| obj.remove("env"));
            }
        }
        write_json_pretty(path, &json)?;
        Ok(())
    }

    /// mcp.json 是 GUI 与 headless 共享的原子文件:整段读取→改写必须在跨进程
    /// 文件锁内完成(#521),否则两个进程各自按旧快照写回会互相丢更新。锁只包住
    /// mcp.json 这一段(mcp.lock 是叶子锁,段内的凭据库读写不取其他市场锁);
    /// 同一迁移对 manifest.json 的改写不在 #521 的三个共享状态文件之列,保持原样。
    fn migrate_mcp_json_file(&self, path: &std::path::Path) -> Result<(), String> {
        with_mcp_json_lock(|| self.migrate_mcp_json_file_locked(path))
    }

    fn migrate_mcp_json_file_locked(&self, path: &std::path::Path) -> Result<(), String> {
        // A planted FIFO swapped in after the caller's `is_file()` probe
        // blocks `open()` forever while HOLDING mcp.lock, wedging every
        // mcp.json reader/writer in every process sharing the home. The
        // hardened primitive refuses non-regular targets without blocking;
        // NotFound (raced away after the probe) stays a no-op like the
        // absent file.
        let Some(content) = read_present_private_data_file(path)? else {
            return Ok(());
        };
        let mut json: serde_json::Value = serde_json::from_str(&content)
            .map_err(|e| format!("解析 {} 失败: {e}", path.display()))?;
        let mut changed = false;
        let Some(servers) = json
            .get_mut("servers")
            .and_then(|servers| servers.as_object_mut())
        else {
            return Ok(());
        };

        for (server_name, entry) in servers.iter_mut() {
            let Some(spec) = legacy_spec_for_server_name(server_name) else {
                continue;
            };
            if let Some(env) = entry.get_mut("env").and_then(|env| env.as_object_mut()) {
                if let Some(value) = env
                    .get(spec.key)
                    .and_then(|v| v.as_str())
                    // 已迁移的条目值就是占位符本身:跳过——
                    // 否则占位符非空、每次启动都在 mcp.lock 内重写同一份
                    // mcp.json,且凭据库被清空时会把占位符文本当明文重新写回。
                    .filter(|v| {
                        !v.trim().is_empty() && v.trim() != mcp_secret_placeholder(spec.key)
                    })
                    .map(ToOwned::to_owned)
                {
                    self.store_migrated_secret(spec, &value)?;
                    env.insert(
                        spec.key.to_string(),
                        serde_json::Value::String(mcp_secret_placeholder(spec.key)),
                    );
                    changed = true;
                }
            }
            if let Some(headers) = entry
                .get_mut("headers")
                .and_then(|headers| headers.as_object_mut())
            {
                if let Some(auth) = headers
                    .get("Authorization")
                    .and_then(|v| v.as_str())
                    .filter(|v| !v.trim().is_empty())
                    .map(ToOwned::to_owned)
                {
                    if let Some(secret) = auth.strip_prefix("Bearer ").filter(|v| !v.is_empty()) {
                        self.store_migrated_secret(spec, secret)?;
                        // `headers` 是字面量,不会展开 `${ENV}`。迁移到
                        // 底座的 Bearer 环境变量字段,避免“已迁移但实际鉴权失败”。
                        headers.remove("Authorization");
                        if headers.is_empty() {
                            entry.as_object_mut().map(|object| object.remove("headers"));
                        }
                        entry["bearer_token_env_var"] =
                            serde_json::Value::String(mcp_secret_env_var(spec.key));
                        changed = true;
                    }
                }
            }
        }

        if changed {
            write_json_pretty(path, &json)?;
        }
        Ok(())
    }

    fn store_migrated_secret(&self, spec: &LegacyMcpSecretSpec, value: &str) -> Result<(), String> {
        let reference = mcp_secret_reference(spec.tool_id, spec.target, spec.key);
        let env_value = match self.credential_store.get(&reference) {
            Ok(Some(existing)) if !existing.trim().is_empty() => {
                log::info!(
                    "[marketplace] MCP 工具 '{}' 的密钥 {} 已存在，已跳过覆盖并清理旧明文",
                    spec.tool_id,
                    spec.key
                );
                existing
            }
            Ok(_) => {
                self.credential_store
                    .set(&reference, value)
                    .map_err(|e| mcp_secret_store_error(spec.tool_id, spec.key, e))?;
                log::info!(
                    "[marketplace] MCP 工具 '{}' 的密钥 {} 已迁移到系统凭据存储",
                    spec.tool_id,
                    spec.key
                );
                value.to_string()
            }
            Err(e) => {
                return Err(mcp_secret_store_error(spec.tool_id, spec.key, e));
            }
        };
        store_secret_value(mcp_secret_env_var(spec.key), env_value);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::legacy_mcp_secret_specs;
    use crate::features::marketplace::{mcp_catalog, secrets};
    /// Every legacy migration spec's key must be enumerable from the tool's
    /// embedded manifest. Otherwise `sync_secret_values` wipes the migrated
    /// credential from the in-process registry on the first restart and the
    /// migrated bearer wiring silently 401s — the qcc manifest once forgot
    /// exactly this declaration.
    #[test]
    fn embedded_manifests_enumerate_every_legacy_migration_secret() {
        for spec in legacy_mcp_secret_specs() {
            let manifest = mcp_catalog::embedded_manifest(spec.tool_id)
                .unwrap()
                .unwrap_or_else(|| panic!("embedded manifest for {}", spec.tool_id));
            let targets = secrets::manifest_secret_targets(&manifest);
            assert!(
                targets.contains(&(spec.target.to_string(), spec.key.to_string())),
                "manifest for '{}' must declare secret {} under target {} so the restart \
                 rehydration keeps it",
                spec.tool_id,
                spec.key,
                spec.target,
            );
        }
    }

    /// An already-migrated entry holds the placeholder
    /// itself. The migration must converge to a no-write — no mcp.json
    /// rewrite under mcp.lock on every boot, and no placeholder-text
    /// round-trip into the credential store (a wiped store used to re-store
    /// the literal placeholder as if it were the secret).
    #[test]
    fn migration_of_placeholder_entries_converges_to_a_no_write() {
        crate::platform::test_support::with_temp_home("pinvou3-mcp-migration-converge", || {
            use crate::features::marketplace::{MarketplaceManager, secrets};
            use crate::platform::credential_store::{CredentialStore as _, MemoryCredentialStore};
            let mcp_path = crate::platform::paths::mcp_config_path();
            std::fs::create_dir_all(mcp_path.parent().unwrap()).unwrap();
            let placeholder = secrets::mcp_secret_placeholder("AMAP_KEY");
            let original =
                format!(r#"{{"servers":{{"weather":{{"env":{{"AMAP_KEY":"{placeholder}"}}}}}}}}"#);
            std::fs::write(&mcp_path, &original).unwrap();
            let mgr = MarketplaceManager::with_store(MemoryCredentialStore::default());

            mgr.migrate_mcp_json_file(&mcp_path).unwrap();

            let after = std::fs::read_to_string(&mcp_path).unwrap();
            assert_eq!(
                after, original,
                "an already-migrated entry must not be rewritten"
            );
            let reference = secrets::mcp_secret_reference("weather", "env", "AMAP_KEY");
            assert!(
                mgr.credential_store.get(&reference).unwrap().is_none(),
                "the placeholder text must not be stored as the secret"
            );
        });
    }

    /// The read INSIDE the mcp.lock critical
    /// section (`migrate_mcp_json_file` → `_locked`) must refuse a planted
    /// FIFO through the hardened primitive instead of blocking `open()`
    /// forever while HOLDING the cross-process mcp.lock (the caller's
    /// `is_file()` probe only narrows this to a swap-in window — the raw
    /// regression wedges every mcp.json reader/writer in every process
    /// sharing the home). The bounded worker + abort containment mirror the
    /// `migrate_mcp_json_paths` pin: a regression fails loudly and cannot
    /// cascade-hang the serial lane on the held mcp.lock.
    #[test]
    #[cfg(unix)]
    fn mcp_secret_migration_locked_read_refuses_a_planted_fifo() {
        crate::platform::test_support::with_temp_home("pinvou3-mcp-migration", || {
            // cfg(unix)-only: these two are used by this test alone, so they
            // live here to keep `cargo check --all-targets` warning-free on
            // Windows (unused_imports is denied).
            use crate::features::marketplace::MarketplaceManager;
            use crate::platform::credential_store::MemoryCredentialStore;
            let mcp_path = crate::platform::paths::mcp_config_path();
            std::fs::create_dir_all(mcp_path.parent().unwrap()).unwrap();
            crate::platform::paths::tests::plant_fifo(&mcp_path);
            let mgr = MarketplaceManager::with_store(MemoryCredentialStore::default());

            let result = crate::platform::test_support::run_bounded_worker_or_abort(
                "the mcp secret migration still blocks on a planted mcp.json FIFO \
                 while holding mcp.lock — the deadlock also wedges real users",
                move || mgr.migrate_mcp_json_file(&mcp_path),
            );
            let error = result.expect_err("a planted FIFO must fail the locked migration read");
            assert!(
                error.contains("mcp.json"),
                "the refusal must name the target: {error}"
            );
        });
    }
}
