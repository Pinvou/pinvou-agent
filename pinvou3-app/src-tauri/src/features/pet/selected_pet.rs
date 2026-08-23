use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tauri::{AppHandle, Emitter, State};

const BUILTIN_PET_IDS: [&str; 3] = ["lingling", "langlang", "ace-taffy"];
const DEFAULT_PET_ID: &str = "lingling";
const SELECTED_PET_CHANGED_EVENT: &str = "pet:selected_changed";

#[derive(Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
struct SelectedPetFile {
    selected_pet: String,
}

fn normalize_selected_pet(id: Option<&str>) -> &'static str {
    id.and_then(|candidate| {
        BUILTIN_PET_IDS
            .iter()
            .copied()
            .find(|known| *known == candidate)
    })
    .unwrap_or(DEFAULT_PET_ID)
}

fn load_selected_pet(path: &Path) -> String {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<SelectedPetFile>(&raw).ok())
        .map(|saved| normalize_selected_pet(Some(&saved.selected_pet)))
        .unwrap_or(DEFAULT_PET_ID)
        .to_string()
}

pub struct SelectedPetStore {
    path: PathBuf,
    selected_pet: Mutex<String>,
}

impl SelectedPetStore {
    pub fn load() -> Self {
        let path = crate::platform::paths::pinvou3_home().join("selected_pet.json");
        let selected_pet = load_selected_pet(&path);
        Self {
            path,
            selected_pet: Mutex::new(selected_pet),
        }
    }

    fn get(&self) -> String {
        self.selected_pet
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn persist(&self, id: &str) -> Result<(), String> {
        let payload = match serde_json::to_vec(&SelectedPetFile {
            selected_pet: id.to_string(),
        }) {
            Ok(payload) => payload,
            Err(error) => return Err(format!("serialize selected pet failed: {error}")),
        };
        self.write_payload(&payload)
    }

    fn write_payload(&self, payload: &[u8]) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("create selected pet directory failed: {error}"))?;
        }
        deepseek_tui::utils::write_atomic(&self.path, payload)
            .map_err(|error| format!("write selected pet failed: {error:#}"))
    }

    fn set_and_notify<F>(
        &self,
        id: &str,
        expected_current: Option<&str>,
        notify: F,
    ) -> Result<(), String>
    where
        F: FnOnce() -> Result<(), String>,
    {
        if !BUILTIN_PET_IDS.contains(&id) {
            return Err(format!("unknown pet id: {id}"));
        }

        let mut selected_pet = self
            .selected_pet
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // CAS 语义：补偿性写入(激活失败回滚/启动收敛)必须声明它要替换的
        // 当前值。若期间用户已做出新选择,过期的补偿写会静默吞掉用户意图,
        // 这里直接拒绝——正常的用户选择不带 expected_current,无条件生效。
        if let Some(expected) = expected_current {
            if selected_pet.as_str() != expected {
                return Err(format!(
                    "stale selection update ignored: current is {}, expected {expected}",
                    *selected_pet
                ));
            }
        }
        let previous = selected_pet.clone();
        self.persist(id)?;
        *selected_pet = id.to_string();
        if let Err(error) = notify() {
            // 广播失败则回滚磁盘与内存：命令返回 Err 时前端会停留在旧宠物,
            // 三层(磁盘/内存/UI)必须一致,否则重启会"突然"切到新宠物。
            // 回滚写盘失败只能尽力而为——此时磁盘=新值,重启后收敛到新宠物,
            // 属于有界的降级,不再额外分叉。
            let _ = self.persist(&previous);
            *selected_pet = previous;
            return Err(error);
        }
        Ok(())
    }
}
pub fn get_selected_pet(store: State<'_, SelectedPetStore>) -> String {
    store.get()
}
pub fn set_selected_pet(
    id: String,
    expected_current: Option<String>,
    store: State<'_, SelectedPetStore>,
    app: AppHandle,
) -> Result<(), String> {
    store.set_and_notify(&id, expected_current.as_deref(), || {
        app.emit(
            SELECTED_PET_CHANGED_EVENT,
            serde_json::json!({ "selected_pet": id }),
        )
        .map_err(|error| format!("emit selected pet change failed: {error}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);

    struct TempHome {
        path: PathBuf,
        previous: Option<OsString>,
    }

    impl TempHome {
        fn enter(label: &str) -> Self {
            let unique = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "pinvou3-selected-pet-{label}-{}-{unique}",
                std::process::id()
            ));
            let previous = std::env::var_os("PINVOU3_HOME");
            // SAFETY: 持 platform::paths::tests::ENV_LOCK,进程内 env 写已串行化。
            unsafe { std::env::set_var("PINVOU3_HOME", &path) };
            Self { path, previous }
        }
    }

    impl Drop for TempHome {
        fn drop(&mut self) {
            match self.previous.take() {
                // SAFETY: 持 platform::paths::tests::ENV_LOCK,进程内 env 写已串行化。
                Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
                // SAFETY: 持 platform::paths::tests::ENV_LOCK,进程内 env 写已串行化。
                None => unsafe { std::env::remove_var("PINVOU3_HOME") },
            }
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn whitelist_normalization_covers_known_missing_and_unknown_ids() {
        for id in BUILTIN_PET_IDS {
            assert_eq!(normalize_selected_pet(Some(id)), id);
        }
        for id in [None, Some(""), Some("Lingling"), Some("unknown")] {
            assert_eq!(normalize_selected_pet(id), DEFAULT_PET_ID);
        }
    }

    #[test]
    fn selected_pet_file_roundtrips() {
        let saved = SelectedPetFile {
            selected_pet: "langlang".to_string(),
        };
        let json = serde_json::to_string(&saved).expect("serialize selected pet");
        assert_eq!(
            serde_json::from_str::<SelectedPetFile>(&json).expect("deserialize selected pet"),
            saved
        );
    }

    #[test]
    fn missing_file_falls_back_to_default_without_writing() {
        let _guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let home = TempHome::enter("missing");
        let selected_path = home.path.join("selected_pet.json");

        let store = SelectedPetStore::load();

        assert_eq!(store.get(), DEFAULT_PET_ID);
        assert!(!selected_path.exists());
    }

    #[test]
    fn invalid_payloads_fall_back_without_rewriting() {
        let _guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let home = TempHome::enter("invalid");
        std::fs::create_dir_all(&home.path).expect("create temp home");
        let selected_path = home.path.join("selected_pet.json");

        for payload in [
            "{not-json",
            r#"{"wrong_field":"langlang"}"#,
            r#"{"selected_pet":"outsider"}"#,
        ] {
            std::fs::write(&selected_path, payload).expect("write invalid selected pet");
            let store = SelectedPetStore::load();
            assert_eq!(store.get(), DEFAULT_PET_ID);
            assert_eq!(
                std::fs::read_to_string(&selected_path).expect("invalid file remains"),
                payload
            );
        }
    }

    #[test]
    fn notify_failure_rolls_back_disk_and_memory() {
        let _guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let home = TempHome::enter("notify-failure");
        let selected_path = home.path.join("selected_pet.json");
        let store = SelectedPetStore::load();
        store
            .set_and_notify("ace-taffy", None, || Ok(()))
            .expect("seed a committed selection");

        let error = store
            .set_and_notify("langlang", None, || Err("emit failed".to_string()))
            .expect_err("notify failure must surface");

        assert!(error.contains("emit failed"));
        assert_eq!(store.get(), "ace-taffy", "memory must roll back");
        assert_eq!(
            load_selected_pet(&selected_path),
            "ace-taffy",
            "disk must roll back"
        );
    }

    #[test]
    fn stale_compensating_write_is_rejected_without_side_effects() {
        let _guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let home = TempHome::enter("stale-cas");
        let selected_path = home.path.join("selected_pet.json");
        let store = SelectedPetStore::load();
        store
            .set_and_notify("ace-taffy", None, || Ok(()))
            .expect("user picks ace-taffy");
        let mut notified = false;

        // 过期的补偿写：以为当前还是 langlang(失败的切换目标),想回滚到
        // lingling——但用户已经选了 ace-taffy,必须拒绝且零副作用。
        let error = store
            .set_and_notify("lingling", Some("langlang"), || {
                notified = true;
                Ok(())
            })
            .expect_err("stale CAS must fail");

        assert!(error.contains("stale selection update"));
        assert!(!notified);
        assert_eq!(store.get(), "ace-taffy");
        assert_eq!(load_selected_pet(&selected_path), "ace-taffy");

        // 期望值匹配时正常生效。
        store
            .set_and_notify("lingling", Some("ace-taffy"), || Ok(()))
            .expect("matching CAS succeeds");
        assert_eq!(store.get(), "lingling");
    }

    #[test]
    fn unknown_id_is_rejected_without_writing_or_notifying() {
        let _guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let home = TempHome::enter("unknown");
        let selected_path = home.path.join("selected_pet.json");
        let store = SelectedPetStore::load();
        let mut notified = false;

        let error = store
            .set_and_notify("outsider", None, || {
                notified = true;
                Ok(())
            })
            .expect_err("unknown id must fail");

        assert!(error.contains("unknown pet id"));
        assert_eq!(store.get(), DEFAULT_PET_ID);
        assert!(!selected_path.exists());
        assert!(!notified);
    }
}
