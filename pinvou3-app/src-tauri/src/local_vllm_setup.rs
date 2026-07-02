//! MegaCube(GB10) 本地大模型一键引导。
//!
//! 整机出厂会在 `/opt/.h3c/packages/vllm/` 预装推理引擎容器压缩包 + `models/` 下
//! 模型压缩包 + `startup.sh` 可执行启动脚本,但开箱后并未拉起。本模块负责:
//!  1. `detect_local_vllm_setup` —— 首屏检测"预装但未启用"状态(预装齐全+未在线+没引导过 → 弹框)。
//!  2. `bootstrap_local_vllm` —— 用户点"启用"后,把 startup.sh 装成 systemd 服务拉起引擎+
//!     开机自启、探到就绪后把本地 vLLM 写进模型配置设为默认。
//!
//! 设计要点:
//! - 触发不以机型(DMI product_name)为门槛:GB10 底层是 NVIDIA DGX Spark,H3C 不同批次固件
//!   DMI 串写法不一(实测 `SCI-CUBE` / `NVIDIA_DGX_Spark`),易漏判。改以 `/opt/.h3c/packages/vllm`
//!   这套预装为真凭据——出厂镜像独有,普通机不会有。product_name 仅回填 `is_megacube` 供诊断。
//! - 提权只发生在引导阶段、且**一次 pkexec** 办完(给 startup.sh 加可执行位+装单元+reload+enable --now),
//!   复用 `super_permission::run_pkexec` 的 pkexec 范式;**不走** sudoers 开关,二者无关。
//! - startup.sh 以 root 跑,内容来自出厂预装目录——属可信来源。该脚本须可重复
//!   执行(docker load 幂等 / docker run 固定 --name 前先 docker rm -f),是预装侧编写约定。

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::Emitter;

use crate::bridge::prefs::{ModelPreset, SavedModel, UserPrefs};
use crate::credential_store::CredentialState;
use crate::monitor::{self, VllmStatus};

/// 引导阶段事件名(前端 listen 更新步骤指示 + 计时)。
const PHASE_EVENT: &str = "vllm-setup:phase";

const H3C_VLLM_DIR: &str = "/opt/.h3c/packages/vllm";
const STARTUP_SH: &str = "/opt/.h3c/packages/vllm/startup.sh";
const MODELS_DIR: &str = "/opt/.h3c/packages/vllm/models";
const SERVICE_NAME: &str = "pinvou3-vllm.service";
const PRODUCT_NAME_PATH: &str = "/sys/class/dmi/id/product_name";
const VLLM_PORTS: [u16; 3] = [8000, 8001, 8002];
/// 探到在线但 /v1/models 没回模型名时的兜底(正常 vLLM 一定有 served name,极少用到)。
const FALLBACK_MODEL: &str = "qwen36_35b_256k";
/// 引导成功后写入的固定模型 id(再次引导按 id upsert,幂等)。
const BOOTSTRAP_MODEL_ID: &str = "local-vllm-megacube";

#[derive(Debug, Clone, Serialize)]
pub struct LocalVllmSetupStatus {
    /// eligible:预装包齐全 + 本地 vLLM 未在线 + 没成功跑过引导。机型仅作诊断,不进门槛。
    pub eligible: bool,
    pub is_megacube: bool,
    pub has_packages: bool,
    pub vllm_online: bool,
    pub already_bootstrapped: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct BootstrapResult {
    pub base_url: String,
    pub model: String,
}

/// 归一化 product_name 后判型:去空白、转大写、剔除非字母数字(吸收 `-`/`_`/空格差异)。
/// GB10 底层是 NVIDIA DGX Spark 参考平台,H3C 不同批次固件 DMI 串写法不一:实测有
/// `SCI-CUBE`(113 真机)与 `NVIDIA_DGX_Spark`(cube-bb89 真机)两种,故认这一组已知串。
/// ⚠️ 仅供 `is_megacube` 诊断展示——eligible **不再**以机型为门槛(见 detect_local_vllm_setup),
/// 故即便以后冒出第三种串、这里返 false,也不影响引导框触发。
fn product_is_megacube(raw: &str) -> bool {
    let norm: String = raw
        .trim()
        .to_ascii_uppercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    matches!(norm.as_str(), "SCICUBE" | "NVIDIADGXSPARK")
}

/// 读 `/sys/class/dmi/id/product_name` 判机型。
fn is_megacube() -> bool {
    std::fs::read_to_string(PRODUCT_NAME_PATH)
        .map(|s| product_is_megacube(&s))
        .unwrap_or(false)
}

/// 预装三件齐全:startup.sh 存在 + models/ 非空 + vllm/ 下有引擎压缩包。
/// startup.sh 是预装侧提供的可执行启动脚本,bootstrap 直接拿它当 systemd ExecStart。
fn has_packages() -> bool {
    Path::new(STARTUP_SH).is_file() && dir_non_empty(MODELS_DIR) && has_engine_archive(H3C_VLLM_DIR)
}

fn dir_non_empty(dir: &str) -> bool {
    std::fs::read_dir(dir)
        .map(|mut rd| rd.next().is_some())
        .unwrap_or(false)
}

/// dir 下是否有常见压缩包(引擎容器包)。`.tar.gz` 以 `.gz` 结尾故已覆盖。
fn has_engine_archive(dir: &str) -> bool {
    const EXTS: [&str; 5] = ["tar", "gz", "tgz", "zip", "7z"];
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten().any(|e| {
                let p = e.path();
                p.is_file()
                    && p.file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| {
                            let lower = n.to_ascii_lowercase();
                            EXTS.iter().any(|ext| lower.ends_with(&format!(".{ext}")))
                        })
                        .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

/// 探本机 vLLM 候选端口,返回首个在线实例 (base_url, served_model)。
/// 在线 = `/v1/models` 有响应(status 非 Offline,Mismatch 也算在线)。
async fn probe_online() -> Option<(String, Option<String>)> {
    for port in VLLM_PORTS {
        let base = format!("http://127.0.0.1:{port}/v1");
        if let Some(snap) = monitor::vllm_snapshot(&base, None).await {
            if !matches!(snap.status, VllmStatus::Offline) {
                return Some((snap.upstream, snap.model));
            }
        }
    }
    None
}

fn systemd_unit() -> String {
    format!(
        "[Unit]\n\
         Description=pinvou3 local vLLM inference engine (MegaCube)\n\
         After=network-online.target docker.service\n\
         Wants=network-online.target\n\
         \n\
         [Service]\n\
         Type=oneshot\n\
         RemainAfterExit=yes\n\
         WorkingDirectory={H3C_VLLM_DIR}\n\
         ExecStart={STARTUP_SH}\n\
         \n\
         [Install]\n\
         WantedBy=multi-user.target\n"
    )
}

/// 跑 pkexec 并捕获输出(失败时把 stderr 带回前端)。
/// 退出码约定同 super_permission:126 用户取消 / 127 未授权或 pkexec 不可用。
fn run_pkexec_capture(args: &[&str]) -> Result<(), String> {
    let out = Command::new("pkexec")
        .args(args)
        .output()
        .map_err(|e| format!("pkexec 启动失败: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let code = out.status.code().unwrap_or(-1);
    let stderr = String::from_utf8_lossy(&out.stderr);
    Err(match code {
        126 => "用户取消了授权".to_string(),
        127 => "未授权或 pkexec 不可用".to_string(),
        _ => format!("启动脚本执行失败 (exit {code}): {}", stderr.trim()),
    })
}

/// 首屏检测:预装齐全 + 本地 vLLM 未在线 + 没成功引导过 + 没婉拒过 → eligible(前端据此弹引导框)。
///
/// 机型不再作硬门槛——`/opt/.h3c/packages/vllm` 这套预装是 H3C 出厂镜像独有的强凭据,比 DMI
/// product_name 可靠(同型号不同批次 DMI 串都不一样)。`is_megacube` 仍回填供诊断。
/// 端口探测只在预装齐全、没跑过引导、没婉拒时才做——普通机(无预装)直接短路,不白等 3 个端口各 3s 超时。
/// 注:`declined` 只压住**开机自动弹框**;设置页「检测本机 vLLM」仍据 `has_packages` 提供手动启用入口。
#[tauri::command]
pub async fn detect_local_vllm_setup() -> Result<LocalVllmSetupStatus, String> {
    let is_megacube = is_megacube();
    let has_packages = has_packages();
    let prefs = UserPrefs::load();
    let already_bootstrapped = prefs.advanced.local_vllm_bootstrapped;
    let declined = prefs.advanced.local_vllm_setup_declined;

    let worth_probing = has_packages && !already_bootstrapped && !declined;
    let vllm_online = if worth_probing {
        probe_online().await.is_some()
    } else {
        false
    };
    let eligible = has_packages && !vllm_online && !already_bootstrapped && !declined;

    Ok(LocalVllmSetupStatus {
        eligible,
        is_megacube,
        has_packages,
        vllm_online,
        already_bootstrapped,
    })
}

/// 用户在引导框点「不再提醒 → 确认」:持久置 declined,开机引导框不再自动弹。
/// 不影响设置页「检测本机 vLLM」的手动启用入口(那条按 has_packages 提供,与 declined 无关)。
#[tauri::command]
pub fn decline_local_vllm_setup() -> Result<(), String> {
    let mut prefs = UserPrefs::load();
    prefs.advanced.local_vllm_setup_declined = true;
    prefs.save().map_err(|e| format!("保存失败: {e:?}"))?;
    Ok(())
}

/// 用户点"启用"后执行:把预装 startup.sh 装成 systemd 服务(一次 pkexec、enable --now)
/// → 轮询就绪 → 写模型配置设默认 + 置 bootstrapped 标记。
///
/// 全程发 `vllm-setup:phase` 事件(authorizing → waiting{attempt} → ready),
/// 前端据此显示步骤指示;计时由前端自跑(pkexec 阻塞期间也能看到秒数在涨)。
#[tauri::command]
pub async fn bootstrap_local_vllm(app: tauri::AppHandle) -> Result<BootstrapResult, String> {
    // 1. 重校验(detect 与点击之间状态可能变)。机型不卡,只认预装齐全。
    if !has_packages() {
        return Err("未找到完整的本地大模型预装包(startup.sh / 引擎包 / models)。".to_string());
    }

    // 2. 写 systemd 单元到临时文件(用户态),交 pkexec 用 install 提权落到系统路径。
    //    预装 startup.sh 本身就是可执行启动脚本,直接当 ExecStart——不再解析/转写,也不生成中间脚本。
    //    `cd` 由 systemd WorkingDirectory 提供;shebang / `set -euo pipefail` 由 startup.sh 自带。
    let tmp_unit = std::env::temp_dir().join("pinvou3-vllm.service");
    std::fs::write(&tmp_unit, systemd_unit()).map_err(|e| format!("写服务单元失败: {e}"))?;

    // 3. 一次 pkexec:给 startup.sh 加可执行位 + 装单元 → daemon-reload → enable --now(现在拉起 + 开机自启)。
    //    pkexec 同步阻塞(含密码框 + docker load 大镜像),期间前端靠自跑计时显示「在动」。
    let _ = app.emit(PHASE_EVENT, serde_json::json!({ "phase": "authorizing", "attempt": 0 }));
    let script = "set -e\n\
        chmod +x /opt/.h3c/packages/vllm/startup.sh\n\
        install -m 0644 -o root -g root \"$1\" /etc/systemd/system/pinvou3-vllm.service\n\
        systemctl daemon-reload\n\
        systemctl enable --now pinvou3-vllm.service";
    let tu = tmp_unit.to_string_lossy().to_string();
    run_pkexec_capture(&["bash", "-c", script, "pinvou3", &tu])?;
    let _ = std::fs::remove_file(&tmp_unit);

    // 4. 健康轮询,每轮发 waiting{attempt} 事件。
    //    超时给 10 分钟:113 真机实测 GB10 NVFP4 首启 ~5-6min(权重 117s + torch.compile 33s
    //    + flashinfer autotune + cudagraph capture),5 分钟会刚好误杀;二次启动有
    //    ~/.cache/vllm 编译/autotune 缓存会快很多,故 600s 仅首启用得满。
    let _ = app.emit(PHASE_EVENT, serde_json::json!({ "phase": "waiting", "attempt": 0 }));
    let deadline = Instant::now() + Duration::from_secs(600);
    let mut attempt: u32 = 0;
    let (base_url, served) = loop {
        if let Some(found) = probe_online().await {
            break found;
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "推理引擎已拉起,但 10 分钟内未就绪。可稍后在设置里手动探测,或终端查看 `systemctl status {SERVICE_NAME}`。"
            ));
        }
        attempt += 1;
        let _ = app.emit(PHASE_EVENT, serde_json::json!({ "phase": "waiting", "attempt": attempt }));
        tokio::time::sleep(Duration::from_secs(3)).await;
    };
    let _ = app.emit(PHASE_EVENT, serde_json::json!({ "phase": "ready", "attempt": attempt }));
    let model = served.unwrap_or_else(|| FALLBACK_MODEL.to_string());

    // 5. 写模型配置 + 设默认 + 置标记。
    let mut prefs = UserPrefs::load();
    prefs.upsert_model(SavedModel {
        id: BOOTSTRAP_MODEL_ID.to_string(),
        name: "本地大模型".to_string(),
        preset: ModelPreset::LocalVllm,
        model: model.clone(),
        base_url: base_url.clone(),
        api_key: String::new(),
        // 本地 vLLM 无 key,凭证字段同 prefs.rs 内置模型的无 key 缺省
        credential_ref: None,
        credential_state: CredentialState::Missing,
        has_secret: false,
        credential_action: None,
    });
    prefs.advanced.active_model_id = Some(BOOTSTRAP_MODEL_ID.to_string());
    prefs.advanced.local_vllm_bootstrapped = true;
    prefs.save().map_err(|e| format!("保存模型配置失败: {e:?}"))?;

    Ok(BootstrapResult { base_url, model })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn product_is_megacube_accepts_separator_variants() {
        assert!(product_is_megacube("SCI-CUBE")); // 113 真机实测(连字符)
        assert!(product_is_megacube("SCI_CUBE")); // 下划线写法
        assert!(product_is_megacube("  SCI-CUBE\n")); // 带空白/换行
        assert!(product_is_megacube("sci cube")); // 大小写/空格
        assert!(product_is_megacube("NVIDIA_DGX_Spark")); // cube-bb89 真机实测(GB10=DGX Spark)
        assert!(product_is_megacube("NVIDIA DGX Spark")); // 空格写法
        assert!(!product_is_megacube("System Product Name"));
        assert!(!product_is_megacube("SCI-CUBE-PRO")); // 非精确同名不误判
        assert!(!product_is_megacube(""));
    }

    #[test]
    fn systemd_unit_has_required_fields() {
        let u = systemd_unit();
        assert!(u.contains("Type=oneshot"));
        assert!(u.contains("RemainAfterExit=yes"));
        assert!(u.contains("ExecStart=/opt/.h3c/packages/vllm/startup.sh"));
        assert!(u.contains("WantedBy=multi-user.target"));
    }
}
