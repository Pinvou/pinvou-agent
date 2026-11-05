// architecture-guard: allow-target-cfg -- This native Wry test uses Windows any-thread event-loop creation and verifies Windows cwd handle denial; portable domain tests remain separate.
#![cfg(windows)]

use super::*;
use crate::features::assistant::engine_pool::EnginePool;
use crate::features::marketplace::{ConnectorScope, recycle_bin::RecycleBin, scope};
use crate::features::sessions::SessionStore;
use std::io::Write;
use std::sync::Arc;
use tauri::Manager;

fn package_bytes(id: &str, with_skill: bool) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    let plugin = serde_json::json!({
        "manifest_version": 1, "id": id, "name": "Native uninstall fixture",
        "components": {"mcp_servers": [{"id": id, "dir": "mcp"}],
                       "skills": if with_skill { vec![serde_json::json!({"id": format!("{id}-skill"), "dir": format!("skills/{id}-skill")})] } else { Vec::new() }}
    });
    let manifest = serde_json::json!({
        "id": id, "name": "Native uninstall fixture", "description": "local fixture",
        "version": "1.0.0", "icon": "", "category": "life", "mcp_tools": [],
        "command": crate::platform::paths::python_command(), "args": ["server.py"],
        "companion_skills": if with_skill { vec![format!("{id}-skill")] } else { Vec::new() }
    });
    let script = r#"import json, os, pathlib, sys
os.chdir(pathlib.Path(__file__).parent)
for line in sys.stdin:
    request = json.loads(line)
    if 'id' not in request: continue
    if request['method'] == 'initialize':
        result = {'protocolVersion':'2024-11-05','capabilities':{'tools':{}},'serverInfo':{'name':'fixture','version':'1'}}
    elif request['method'] == 'tools/list':
        result = {'tools':[{'name':'fixture','description':'local fixture','inputSchema':{'type':'object'}}]}
    else: result = {}
    print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':result}), flush=True)
"#;
    let mut entries = vec![
        (
            "plugin.json".to_string(),
            serde_json::to_vec(&plugin).unwrap(),
        ),
        (
            "mcp/manifest.json".to_string(),
            serde_json::to_vec(&manifest).unwrap(),
        ),
        ("mcp/server.py".to_string(), script.as_bytes().to_vec()),
    ];
    if with_skill {
        entries.push((
            format!("skills/{id}-skill/SKILL.md"),
            format!("---\nname: {id}-skill\ndescription: fixture\n---\noriginal user content")
                .into_bytes(),
        ));
    }
    for (name, bytes) in entries {
        zip.start_file(name, options).unwrap();
        zip.write_all(&bytes).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

#[test]
fn upload_uninstall_native_command_uses_the_real_pool_and_preserves_retry_and_next_use() {
    // Wry needs its event loop to be created on one dedicated thread. No
    // windows, plugins, user application data or external model calls are used.
    std::thread::spawn(|| {
        crate::platform::test_support::with_temp_home("pinvou-native-uninstall", || {
            let mut context = tauri::generate_context!();
            context.config_mut().app.windows.clear();
            context.config_mut().identifier =
                format!("org.pinvou.uninstalltest{}", std::process::id());
            let app = tauri::Builder::<tauri::Wry>::default()
                .any_thread()
                .build(context)
                .unwrap();
            let store = SessionStore::boot().unwrap();
            let pool = EnginePool::new_with_dependencies(
                app.handle().clone(),
                store.clone(),
                Arc::new(|_, _| Vec::new()),
                Arc::new(|_| Vec::new()),
            )
            .unwrap();
            app.manage(store.clone());
            app.manage(pool.clone());
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async {
                use base64::Engine as _;
                let id = "native-upload";
                let bytes = package_bytes(id, true);
                import_plugin_package_bytes_cmd(
                    "fixture.zip".into(),
                    base64::engine::general_purpose::STANDARD.encode(bytes),
                    app.state(),
                )
                .await
                .unwrap();
                scope::save_disabled_bundles_for(ConnectorScope::Plain, &[]).unwrap();
                let home = crate::platform::paths::pinvou3_home();
                let session = store
                    .create_new("deepseek-chat".into(), None, home.clone())
                    .unwrap();
                let engine = pool.get_or_spawn(&session.metadata.id).await.unwrap();
                tokio::time::timeout(std::time::Duration::from_secs(20), async {
                    loop {
                        let snapshot = engine.handle.bootstrap_mcp().await.unwrap().snapshot;
                        if snapshot.servers.iter().any(|server| {
                            server.name == id && server.connected && !server.tools.is_empty()
                        }) {
                            break;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    }
                })
                .await
                .expect("the production config must launch the uploaded local server");
                let package = crate::platform::paths::bundles_root().join(id);
                let before =
                    std::fs::read(package.join(format!("skills/{id}-skill/SKILL.md"))).unwrap();
                let denied = std::fs::rename(&package, home.join("unexpected-move"));
                assert!(
                    denied.is_err(),
                    "Windows cwd denial must really exist before claiming the regression"
                );
                assert!(matches!(
                    denied.unwrap_err().raw_os_error(),
                    Some(5 | 32 | 33)
                ));
                let reservation = pool.reserve_turn(&session.metadata.id).unwrap();
                let busy = uninstall_marketplace_tool(id.into(), app.state())
                    .await
                    .unwrap_err();
                assert_eq!(busy, "marketplace_uninstall_busy");
                assert!(package.join("mcp/server.py").is_file());
                drop(reservation);

                // Force a failure AFTER resource release and rename. The real
                // domain transaction must restore configuration and directory
                // before new engines can observe them.
                let recycle_file = home.join("marketplace/recycle-bin.json");
                let recycle_before = std::fs::read(&recycle_file).ok();
                let mcp_before: serde_json::Value = serde_json::from_slice(
                    &std::fs::read(crate::platform::paths::mcp_config_path()).unwrap(),
                )
                .unwrap();
                std::fs::write(&recycle_file, b"{broken manifest").unwrap();
                let failed = uninstall_marketplace_tool(id.into(), app.state()).await;
                match recycle_before {
                    Some(bytes) => std::fs::write(&recycle_file, bytes).unwrap(),
                    None => std::fs::remove_file(&recycle_file).unwrap(),
                }
                assert!(failed.unwrap_err().contains("读取回收站清单失败"));
                assert_eq!(
                    serde_json::from_slice::<serde_json::Value>(
                        &std::fs::read(crate::platform::paths::mcp_config_path()).unwrap()
                    )
                    .unwrap(),
                    mcp_before
                );
                assert_eq!(
                    std::fs::read(package.join(format!("skills/{id}-skill/SKILL.md"))).unwrap(),
                    before
                );
                assert!(
                    crate::features::marketplace::store::BundleStore::new()
                        .get(id)
                        .unwrap()
                        .is_some()
                );
                let recovered = pool.get_or_spawn(&session.metadata.id).await.unwrap();
                tokio::time::timeout(std::time::Duration::from_secs(20), async {
                    while !recovered
                        .handle
                        .bootstrap_mcp()
                        .await
                        .unwrap()
                        .snapshot
                        .servers
                        .iter()
                        .any(|server| server.name == id && server.connected)
                    {
                        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    }
                })
                .await
                .expect("next use must reconnect the restored plugin after rollback");

                // A separate fd reproduces a peer's native write lock without
                // owning this process's mutex. Its ready marker is sent only
                // after LockFileEx has succeeded. No runtime fence may be held
                // while the real command waits for this recycle-manifest lock.
                let lock_path = crate::features::marketplace::file_lock::lock_path_for(
                    &home.join("marketplace/recycle-bin.json"),
                );
                let (held, ready) = std::sync::mpsc::channel();
                let (release, until_release) = std::sync::mpsc::channel();
                let peer_path = lock_path.clone();
                let peer = std::thread::spawn(move || {
                    crate::features::marketplace::file_lock::with_foreign_write_lock(
                        &peer_path,
                        || {
                            held.send(()).unwrap();
                            let _ = until_release.recv(); // sender drop also cleans up on assertion failure
                        },
                    );
                });
                ready
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap();
                let worker_app = app.handle().clone();
                let pending = tokio::spawn(async move {
                    uninstall_marketplace_tool(id.into(), worker_app.state()).await
                });
                tokio::task::spawn_blocking(move || {
                    crate::features::marketplace::file_lock::wait_until_process_mutex_parked(
                        &lock_path,
                        "native uninstall recycle wait",
                    );
                })
                .await
                .unwrap();
                let cached = tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    pool.get_or_spawn(&session.metadata.id),
                )
                .await;
                let cold_session = store
                    .create_new("deepseek-chat".into(), None, home.clone())
                    .unwrap();
                let cold = tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    pool.get_or_spawn(&cold_session.metadata.id),
                )
                .await;
                pending.abort();
                let _ = pending.await;
                let admission_after_cancel = pool.reserve_turn(&cold_session.metadata.id);
                let admission_was_open = admission_after_cancel.is_ok();
                // Release the real lock and settle its worker before making
                // assertions that could unwind the isolated environment.
                drop(admission_after_cancel);
                release.send(()).unwrap();
                peer.join().unwrap();
                tokio::time::timeout(std::time::Duration::from_secs(45), async {
                    loop {
                        match uninstall_marketplace_tool(id.into(), app.state()).await {
                            Ok(()) => break,
                            Err(error) if error == "marketplace_uninstall_busy" => {
                                tokio::time::sleep(std::time::Duration::from_millis(10)).await
                            }
                            Err(error) => panic!("retry after releasing peer: {error}"),
                        }
                    }
                })
                .await
                .unwrap();
                assert!(
                    cached.is_ok_and(|result| result.is_ok()),
                    "cached work must not wait behind the peer"
                );
                assert!(
                    cold.is_ok_and(|result| result.is_ok()),
                    "cold work must not wait behind the peer"
                );
                assert!(
                    admission_was_open,
                    "dropping the caller must not freeze unrelated admission"
                );
                assert!(!package.exists());
                let recycled = RecycleBin::held_dir(id);
                assert_eq!(
                    std::fs::read(recycled.join(format!("skills/{id}-skill/SKILL.md"))).unwrap(),
                    before
                );
                assert!(
                    RecycleBin::new()
                        .list()
                        .unwrap()
                        .iter()
                        .any(|entry| entry.id == id && entry.kind == "bundle")
                );
                let next = pool.get_or_spawn(&session.metadata.id).await.unwrap();
                assert!(
                    !next
                        .handle
                        .bootstrap_mcp()
                        .await
                        .unwrap()
                        .snapshot
                        .servers
                        .iter()
                        .any(|server| server.name == id)
                );
                uninstall_marketplace_tool(id.into(), app.state())
                    .await
                    .unwrap();

                // A standalone skill exercises the separate two-leaf path.
                // Post-commit scope cleanup may wait on a peer, but must no
                // longer hold runtime maintenance or block a cold engine.
                let skill_id = "native-post-scope";
                import_skill_md_bytes(
                    "fixture.md".into(),
                    base64::engine::general_purpose::STANDARD.encode(format!(
                        "---\nname: {skill_id}\ndescription: fixture\n---\nunique skill content"
                    )),
                    app.state(),
                )
                .await
                .unwrap();
                let skill_package = crate::platform::paths::bundles_root().join(skill_id);
                let (held, ready) = std::sync::mpsc::channel();
                let (release, until_release) = std::sync::mpsc::channel();
                let peer = std::thread::spawn(move || {
                    scope::hold_disabled_bundles_lock_for_test(|| {
                        held.send(()).unwrap();
                        let _ = until_release.recv();
                    });
                });
                ready
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap();
                let worker_app = app.handle().clone();
                let cleaner = tokio::spawn(async move {
                    loop {
                        match uninstall_marketplace_skill(skill_id.into(), worker_app.state()).await
                        {
                            Err(error) if error == "marketplace_uninstall_busy" => {
                                tokio::time::sleep(std::time::Duration::from_millis(10)).await
                            }
                            outcome => return outcome,
                        }
                    }
                });
                let moved = tokio::time::timeout(std::time::Duration::from_secs(20), async {
                    while skill_package.exists() {
                        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    }
                })
                .await;
                let pending_cleanup = !cleaner.is_finished();
                let usable_during_cleanup = tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    pool.get_or_spawn(&session.metadata.id),
                )
                .await;
                cleaner.abort();
                let _ = cleaner.await;
                let admission = pool.reserve_turn(&session.metadata.id);
                let admitted = admission.is_ok();
                drop(admission);
                release.send(()).unwrap();
                peer.join().unwrap();
                // This follows the aborted worker through the same scope
                // serialization, without adding a production test hook.
                tokio::time::timeout(std::time::Duration::from_secs(20), async {
                    while scope::load_disabled_bundles_for(ConnectorScope::Plain)
                        .contains(&skill_id.to_string())
                    {
                        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    }
                })
                .await
                .unwrap();
                assert!(moved.is_ok() && pending_cleanup);
                assert!(usable_during_cleanup.is_ok_and(|outcome| outcome.is_ok()) && admitted);
                assert!(
                    RecycleBin::held_dir(skill_id)
                        .join(format!("skills/{skill_id}/SKILL.md"))
                        .is_file()
                );

                // This is the actual async command boundary, not only the
                // manager fallback: optional mirror failures must not prevent
                // safe unregister-with-directory-preservation for plain MCPs.
                for (index, broken) in ["corrupt", "missing", "unreadable"].into_iter().enumerate()
                {
                    let plain_id = format!("native-mirror-{index}");
                    import_plugin_package_bytes_cmd(
                        "plain.zip".into(),
                        base64::engine::general_purpose::STANDARD
                            .encode(package_bytes(&plain_id, false)),
                        app.state(),
                    )
                    .await
                    .unwrap();
                    let registry = crate::features::marketplace::store::BundleStore::new();
                    let user_file = crate::platform::paths::bundles_root()
                        .join(&plain_id)
                        .join("mcp/server.py");
                    let user_content = std::fs::read(&user_file).unwrap();
                    let path = registry.file_path();
                    let saved = std::fs::read(&path).unwrap();
                    match broken {
                        "corrupt" => std::fs::write(&path, b"{not-json").unwrap(),
                        "missing" => std::fs::remove_file(&path).unwrap(),
                        _ => {
                            std::fs::remove_file(&path).unwrap();
                            std::fs::create_dir(&path).unwrap();
                        }
                    }
                    let outcome = uninstall_marketplace_tool(plain_id.clone(), app.state()).await;
                    if path.is_dir() {
                        std::fs::remove_dir(&path).unwrap();
                    }
                    std::fs::write(&path, saved).unwrap();
                    registry.remove(&plain_id).unwrap();
                    outcome.unwrap();
                    assert_eq!(std::fs::read(&user_file).unwrap(), user_content);
                    assert!(
                        crate::platform::paths::bundles_root()
                            .join(&plain_id)
                            .join("mcp/server.py")
                            .is_file()
                    );
                    assert!(
                        !crate::features::marketplace::MarketplaceManager::new()
                            .installed_ids()
                            .contains(&plain_id)
                    );
                }
                pool.evict(&session.metadata.id).await;
                pool.evict(&cold_session.metadata.id).await;
            });
        });
    })
    .join()
    .unwrap();
}
