//! `~/.pinvou3/` 目录布局解析。
//!
//! pinvou3-app 不读 `~/.deepseek/`（隔离），所有 deepseek-tui 默认会写到
//! 全局/cwd 的字段都映射到这个独立目录树。布局参见 plan「目录布局」一节。
//!
//! `PINVOU3_HOME` 环境变量可整体重定位（主要用于测试）。

use std::path::PathBuf;

/// 用户家目录 `$HOME`，是 pinvou3-app 的 engine workspace 根。
/// AI 通过相对路径访问 → 落在家目录下；通过绝对路径访问 → trust_mode 放行
/// 但敏感子目录由 path filter / instructions 引导拦截。
pub fn user_home_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home)
}

/// `~/.pinvou3/` 根目录。
pub fn pinvou3_home() -> PathBuf {
    if let Ok(custom) = std::env::var("PINVOU3_HOME") {
        return PathBuf::from(custom);
    }
    user_home_dir().join(".pinvou3")
}

pub fn settings_path() -> PathBuf {
    pinvou3_home().join("settings.json")
}

pub fn bundle_root() -> PathBuf {
    pinvou3_home().join("bundle")
}
pub fn bundle_instructions() -> PathBuf {
    bundle_root().join("instructions.md")
}
pub fn bundle_skills_dir() -> PathBuf {
    bundle_root().join("skills")
}
/// 工作流(Harness Loop)定义目录,跟 `skills/` 并列。
/// skill = LLM 挂载的标准附件(pinvou-review-*);workflow = harness 读、LLM 不挂载(h3c-ppt)。
pub fn bundle_workflow_dir() -> PathBuf {
    bundle_root().join("workflow")
}
pub fn bundle_mcp_json() -> PathBuf {
    bundle_root().join("mcp.json")
}
/// `~/.pinvou3/bundle/mcp-servers/` —— pinvou3 内置 MCP server 脚本目录。
pub fn bundle_mcp_servers_dir() -> PathBuf {
    bundle_root().join("mcp-servers")
}
/// present_artifact MCP server 脚本绝对路径(mcp.json 的 args 指向它)。
pub fn bundle_present_artifact_server() -> PathBuf {
    bundle_mcp_servers_dir().join("present_artifact_server.py")
}
pub fn bundle_version_file() -> PathBuf {
    bundle_root().join("VERSION")
}

/// 拉起 python MCP server(present_artifact / pptx 等)用的解释器命令。
///
/// - **Windows**:优先用**随安装包内置**的 python(`python-win/pythonw.exe`,
///   自带 python-pptx、且 `pythonw` 无控制台窗口 → 启动不弹黑框、不依赖用户机器上的
///   python)。解析顺序:`PINVOU3_PYTHON` 环境变量(开发/测试覆盖)→ 与 exe 同级的
///   `python-win/pythonw.exe`(prod 安装目录)→ 回退 PATH 上的 `pythonw`。
/// - **其他平台**(Linux/macOS):用系统 `python3`(Linux 几乎自带;GUI 子进程不弹窗;
///   依赖由 marketplace 的自动 pip 安装)。
pub fn python_command() -> String {
    #[cfg(target_os = "windows")]
    {
        // 1. 显式覆盖(PINVOU3_PYTHON 指向 python(w).exe)
        if let Ok(p) = std::env::var("PINVOU3_PYTHON") {
            if !p.is_empty() && std::path::Path::new(&p).exists() {
                return p;
            }
        }
        // 2. 随 app 打包的内置 python(发布版)
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                let bundled = dir.join("python-win").join("pythonw.exe");
                if bundled.is_file() {
                    return bundled.to_string_lossy().into_owned();
                }
            }
        }
        // 3. 探测系统已装 python(dev 构建 / 未内置 python 时的兜底)。
        //    缺这层会兜底成裸 "pythonw" → 没把 python 加进 PATH 的机器上,
        //    python MCP server(如高德天气)起不来、工具注册不上。
        if let Some(p) = resolve_system_python_windows() {
            return p;
        }
        // 4. 最后兜底,保持原行为
        "pythonw".to_string()
    }
    #[cfg(not(target_os = "windows"))]
    {
        // Linux/mac:优先 python3;只装了 python 的老环境退而求其次。
        if which_in_path("python3") {
            return "python3".to_string();
        }
        if which_in_path("python") {
            return "python".to_string();
        }
        "python3".to_string()
    }
}

/// Windows:在 PATH、常见安装目录(`%LOCALAPPDATA%\Programs\Python\Python3x`、
/// `%ProgramFiles%\Python3x`)、py 启动器里找一个真实可用的解释器,优先 `pythonw.exe`(无窗口)。
/// 返回绝对路径;都找不到返回 None。
#[cfg(target_os = "windows")]
fn resolve_system_python_windows() -> Option<String> {
    use std::path::PathBuf;
    // a) PATH 上的 pythonw.exe / python.exe
    if let Ok(path_var) = std::env::var("PATH") {
        for name in ["pythonw.exe", "python.exe"] {
            for dir in std::env::split_paths(&path_var) {
                let cand = dir.join(name);
                if cand.is_file() {
                    return Some(cand.to_string_lossy().into_owned());
                }
            }
        }
    }
    // b) 常见安装目录,Python3xx 取较高版本
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(la) = std::env::var("LOCALAPPDATA") {
        roots.push(PathBuf::from(la).join("Programs").join("Python"));
    }
    if let Ok(pf) = std::env::var("ProgramFiles") {
        roots.push(PathBuf::from(pf));
    }
    for root in roots {
        if let Ok(rd) = std::fs::read_dir(&root) {
            let mut vers: Vec<PathBuf> = rd
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.is_dir()
                        && p.file_name()
                            .and_then(|n| n.to_str())
                            .map(|n| n.starts_with("Python3"))
                            .unwrap_or(false)
                })
                .collect();
            vers.sort();
            for d in vers.iter().rev() {
                for name in ["pythonw.exe", "python.exe"] {
                    let cand = d.join(name);
                    if cand.is_file() {
                        return Some(cand.to_string_lossy().into_owned());
                    }
                }
            }
        }
    }
    // c) py 启动器(PATH 或 C:\Windows\py.exe)
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let cand = dir.join("py.exe");
            if cand.is_file() {
                return Some(cand.to_string_lossy().into_owned());
            }
        }
    }
    let winpy = PathBuf::from(r"C:\Windows\py.exe");
    if winpy.is_file() {
        return Some(winpy.to_string_lossy().into_owned());
    }
    None
}

/// 非 Windows:命令是否在 PATH 上存在(简单存在性检查,够用于 python3/python 兜底)。
#[cfg(not(target_os = "windows"))]
fn which_in_path(cmd: &str) -> bool {
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            if dir.join(cmd).is_file() {
                return true;
            }
        }
    }
    false
}

pub fn user_root() -> PathBuf {
    pinvou3_home().join("user")
}
pub fn user_instructions() -> PathBuf {
    user_root().join("instructions.md")
}
pub fn user_skills_dir() -> PathBuf {
    user_root().join("skills")
}
/// `~/.pinvou3/user/personas/` —— 用户自创专家卡牌（卡牌池）。每张卡一个
/// `<id>.json`（PersonaCard 序列化）。跟 bundle 内嵌的内置卡分离，**永不被覆写**。
pub fn user_personas_dir() -> PathBuf {
    user_root().join("personas")
}

/// `~/.deepseek/skills/` — DeepSeek-TUI 标准用户 skills 目录,h3c-ppt /
/// skill-creator 这种由 `/skill install` 装的 skill 都在这里。跟
/// [`user_skills_dir`](pinvou3 私有 `~/.pinvou3/user/skills/`) 平行,工作流
/// 视图 list_skills_v2 把两个目录合并去重展示 (user 覆盖 deepseek 覆盖 bundle)。
pub fn deepseek_skills_dir() -> PathBuf {
    user_home_dir().join(".deepseek").join("skills")
}

/// 兼容字段：阶段 B 旧 sandbox workspace（已不作为 engine workspace 使用，
/// 但保留作为 "AI 私人沙盒" 兜底——某些场景如 monitor 测试还在用）。
pub fn workspace_dir() -> PathBuf {
    pinvou3_home().join("workspace")
}
pub fn notes_path() -> PathBuf {
    pinvou3_home().join("notes.md")
}
pub fn memory_path() -> PathBuf {
    pinvou3_home().join("memory.md")
}
pub fn mcp_config_path() -> PathBuf {
    bundle_mcp_json()
}

/// `~/.pinvou3/sessions/` —— 所有对话历史落盘的根目录。
pub fn sessions_root() -> PathBuf {
    pinvou3_home().join("sessions")
}

/// `~/.pinvou3/workflows/` —— 工作流 run 第一公民的根目录（独立于 sessions/）。
pub fn workflows_root() -> PathBuf {
    pinvou3_home().join("workflows")
}

/// `~/.pinvou3/workflows/<run_id>/` —— 单个 run 的家（run.json + project/）。
pub fn workflow_run_dir(run_id: &str) -> PathBuf {
    workflows_root().join(run_id)
}

/// `~/.pinvou3/workflows/<run_id>/project/` —— 项目目录 = 该 run 的 engine workspace 本身。
pub fn workflow_project_dir(run_id: &str) -> PathBuf {
    workflow_run_dir(run_id).join("project")
}

/// `~/.pinvou3/workflows/index.json` —— 台账。纯缓存可丢弃，决策读取须与 _state 互证。
pub fn workflows_index_path() -> PathBuf {
    workflows_root().join("index.json")
}

/// `~/.pinvou3/updates/` —— 应用内升级下载的 deb 暂存目录。
/// 不用 /tmp：tmpfs 受内存限制 + 重启清空（下载完提示重启后文件就没了）。
pub fn updates_dir() -> PathBuf {
    pinvou3_home().join("updates")
}

/// `~/.pinvou3/sessions/<session_id>/artifacts/` —— AI 默认产物落地目录。
/// `$PINVOU3_SESSION_ARTIFACTS` 环境变量注入这个值给 engine + LLM。
pub fn session_artifacts_dir(session_id: &str) -> PathBuf {
    sessions_root().join(session_id).join("artifacts")
}

/// `~/.pinvou3/sessions/<session_id>/workspace/` —— 每个 session 独立的工作目录。
/// engine workspace 跟随当前 active session 切换，避免多 session 共享文件冲突。
/// 切换 session 时 bridge 调 `Op::SyncSession { workspace }` 重置。
pub fn session_workspace_dir(session_id: &str) -> PathBuf {
    sessions_root().join(session_id).join("workspace")
}

/// `~/.pinvou3/sessions/<session_id>/instructions.md` —— 每个 session 独立的
/// Legacy `~/.pinvou3/sessions/<sid>/instructions.md` 路径。
///
/// C 方案(P-no-disk)前用作 per-session prompt 文件,EngineConfig.instructions
/// 指向它。改成 `InstructionSource::Inline` 后这个 disk 文件**不再被生产代码读**,
/// 仅用于 boot 时 legacy 清理(早期 pinvou3 版本写下的残留)。新版 pinvou3 不再写。
pub fn session_instructions_path(session_id: &str) -> PathBuf {
    sessions_root().join(session_id).join("instructions.md")
}

/// `~/.pinvou3/sessions/<session_id>/persona_events.json` —— 该 session 的卡牌
/// 加持/卸下事件时间线(sidecar)。**刻意独立于 messages**:messages 在 engine
/// 冷启动时会被 sync_session 注水回 LLM,而卡牌事件是纯前端展示,绝不能进 LLM 上下文。
/// 前端按 `pos`(事件发生时的 messages 数)在 rerenderFromMessages 里插回原位。
pub fn session_persona_events(session_id: &str) -> PathBuf {
    sessions_root().join(session_id).join("persona_events.json")
}

/// `~/.pinvou3/sessions/<session_id>/pinvou_reviews.json` —— 该 session 的 Pinvou
/// 召唤检阅时间线（每条 {pos, review}）。同 persona_events 一样**刻意独立于 messages**:
/// 审查卡是纯前端展示、绝不能进 LLM 上下文（那会污染主 AI），前端按 `pos` 在
/// rerenderFromMessages 里插回。Boss 要主 AI 看审阅,走「转交」按钮发成 Boss 消息。
pub fn session_pinvou_reviews(session_id: &str) -> PathBuf {
    sessions_root().join(session_id).join("pinvou_reviews.json")
}

/// 阶段 C 没多 session 时的 fallback artifacts dir（session_id="default"）。
/// Step 4 完成后这个会被切换 session 时动态计算的值替换。
pub fn default_session_artifacts_dir() -> PathBuf {
    session_artifacts_dir("default")
}

/// 首次启动确保所有目录存在。bundle/skills 等子目录在解包时还会再 ensure 一次。
pub fn ensure_dirs() -> std::io::Result<()> {
    std::fs::create_dir_all(bundle_skills_dir())?;
    std::fs::create_dir_all(user_skills_dir())?;
    std::fs::create_dir_all(user_personas_dir())?;
    std::fs::create_dir_all(workspace_dir())?;
    std::fs::create_dir_all(default_session_artifacts_dir())?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Mutex;

    /// 进程级 env var 是测试的硬隔离障碍：cargo test 默认并行跑，多个测试
    /// 同时改 PINVOU3_HOME 会互相覆盖断言。这把全局锁让所有 mutate
    /// `PINVOU3_HOME` 的测试串行执行。bridge::sessions 模块测试也借用这把锁。
    pub(crate) static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn pinvou3_home_respects_env_override() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("PINVOU3_HOME").ok();
        std::env::set_var("PINVOU3_HOME", "/tmp/pinvou3-test-override");
        assert_eq!(pinvou3_home(), PathBuf::from("/tmp/pinvou3-test-override"));
        assert_eq!(
            settings_path(),
            PathBuf::from("/tmp/pinvou3-test-override/settings.json")
        );
        match prev {
            Some(v) => std::env::set_var("PINVOU3_HOME", v),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
    }

    /// `user_home_dir` 应该读 $HOME（pinvou3 engine workspace 之根）。
    #[test]
    fn user_home_dir_reads_home_env() {
        if let Ok(h) = std::env::var("HOME") {
            assert_eq!(user_home_dir(), PathBuf::from(h));
        }
        // 没设 HOME 时 fallback /tmp（不强测，避免 race）
    }

    /// workflow run 目录族必须落在 ~/.pinvou3/workflows/ 下（独立于 sessions/）。
    #[test]
    fn workflow_paths_layout() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("PINVOU3_HOME").ok();
        std::env::set_var("PINVOU3_HOME", "/tmp/pinvou3-wf-paths-test");
        assert_eq!(
            workflows_root(),
            PathBuf::from("/tmp/pinvou3-wf-paths-test/workflows")
        );
        assert_eq!(
            workflow_run_dir("wf-20260610-1432-a3f9"),
            PathBuf::from("/tmp/pinvou3-wf-paths-test/workflows/wf-20260610-1432-a3f9")
        );
        assert_eq!(
            workflow_project_dir("wf-20260610-1432-a3f9"),
            PathBuf::from("/tmp/pinvou3-wf-paths-test/workflows/wf-20260610-1432-a3f9/project")
        );
        assert_eq!(
            workflows_index_path(),
            PathBuf::from("/tmp/pinvou3-wf-paths-test/workflows/index.json")
        );
        match prev {
            Some(v) => std::env::set_var("PINVOU3_HOME", v),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
    }

    /// session artifacts 路径必须落在 ~/.pinvou3/sessions/<id>/artifacts/ 下。
    #[test]
    fn session_artifacts_layout() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("PINVOU3_HOME").ok();
        std::env::set_var("PINVOU3_HOME", "/tmp/pinvou3-artifacts-layout-test");
        assert_eq!(
            session_artifacts_dir("abc123"),
            PathBuf::from("/tmp/pinvou3-artifacts-layout-test/sessions/abc123/artifacts")
        );
        assert_eq!(
            default_session_artifacts_dir(),
            PathBuf::from("/tmp/pinvou3-artifacts-layout-test/sessions/default/artifacts")
        );
        match prev {
            Some(v) => std::env::set_var("PINVOU3_HOME", v),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
    }
}
