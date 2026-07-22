use std::path::{Path, PathBuf};

fn main() {
    // 开发源 workflows/ → resources/common/bundle/(include_dir! 嵌入源)同步。放 build.rs 而非
    // run-dev.sh:任何 cargo build/打包都同步,改完直接 build 也不漂移(run-dev.sh 只覆盖
    // dev 启动)。引擎脚本(_engine/scripts 排 test_*)拷进每个 workflow 的 scripts/,对齐
    // scheduler.py 的 WORKFLOW_ROOT=parent.parent 路径假设。内容比较,源未变不写盘。
    sync_workflow_bundle();

    // 算 bundle 资源的内容 hash，注入到 BUNDLE_INSTRUCTIONS_HASH 环境变量
    // 让 bundle.rs::BUNDLE_VERSION 自动包含 hash —— 文件变化时 cargo build 重跑
    // build.rs，hash 跟着变，ensure_extracted 自动覆写 disk 的 bundle 文件。
    //
    // 纳入哈希的资源（任一变化都要触发老安装重新解包）：
    //   - instructions.md：system prompt 模板
    //   - deny_sensitive_paths.sh：敏感目录/命令硬拦截 hook（只改它也要落盘）
    let mut hashed = Vec::new();
    for f in [
        "resources/common/bundle/instructions.md",
        "resources/common/bundle/deny_sensitive_paths.sh",
    ] {
        println!("cargo:rerun-if-changed={f}");
        hashed.extend(std::fs::read(f).unwrap_or_else(|_| panic!("{f} must exist")));
    }
    let hash = fnv1a_64(&hashed);
    println!("cargo:rustc-env=BUNDLE_INSTRUCTIONS_HASH={hash:016x}");

    // workflow 嵌入目录 hash → env(每个工作流一个)。
    // include_dir! 不发 rerun-if-changed：只改嵌入目录里的文件、增量编译会沿用陈旧嵌入。
    // 这里 hash 整个目录(路径+内容)注入 env，内容一变 BUNDLE_VERSION 就变 → bundle.rs
    // 因引用 env! 而重编 → include_dir! 重读，保证编译嵌入永远新鲜；并对每个文件发
    // rerun-if-changed 触发 build.rs 重跑。
    // (h3c-ppt 已下线存档 2026-06-11,恢复时在此加回一行 hash_dir)
    let sansheng_workflow_hash = hash_dir(Path::new("resources/common/bundle/workflow/sansheng-liubu"));
    println!("cargo:rustc-env=BUNDLE_WORKFLOW_HASH_SANSHENG={sansheng_workflow_hash:016x}");
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").expect("target OS is set by Cargo");
    let target_arch = std::env::var("CARGO_CFG_TARGET_ARCH").expect("target arch is set by Cargo");
    let platform_bundle = PathBuf::from("resources/platforms")
        .join(target_os)
        .join(target_arch)
        .join("bundle");
    let connector_cli_hash = hash_dir(&platform_bundle.join("connectors"));
    println!("cargo:rustc-env=BUNDLE_CONNECTOR_CLI_HASH={connector_cli_hash:016x}");
    let h3c_cli_hash = hash_dir(Path::new("resources/common/bundle/eip"))
        ^ hash_dir(Path::new("resources/common/bundle/zhidao"))
        ^ hash_dir(&platform_bundle.join("eip"))
        ^ hash_dir(&platform_bundle.join("zhidao"));
    println!("cargo:rustc-env=BUNDLE_H3C_CLI_HASH={h3c_cli_hash:016x}");
    tauri_build::build();
}

/// 递归 hash 目录：收集所有文件、按路径排序(确定性)、把相对路径+内容滚进 FNV-1a，
/// 并对每个文件发 cargo:rerun-if-changed。
fn hash_dir(dir: &Path) -> u64 {
    let mut files: Vec<PathBuf> = Vec::new();
    collect_files(dir, &mut files);
    files.sort();
    let mut hash: u64 = 0xcbf29ce484222325;
    for path in &files {
        println!("cargo:rerun-if-changed={}", path.display());
        hash = fnv1a_step(hash, path.to_string_lossy().as_bytes());
        if let Ok(bytes) = std::fs::read(path) {
            hash = fnv1a_step(hash, &bytes);
        }
    }
    hash
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read_dir.flatten() {
        let path = entry.path();
        // __pycache__/.pyc 是跑过引擎测试的机器才有的本地缓存(gitignored 但在盘上),
        // 折进 hash 会让不同机器 hash 漂移→bundle 无谓重解包,且发布物夹 pyc。
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name == "__pycache__" || name.ends_with(".pyc") {
            continue;
        }
        if path.is_dir() {
            collect_files(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// 简易 FNV-1a 64 位 hash —— 不需要 cryptographic 强度，只要内容变化时 hash 变即可。
fn fnv1a_64(bytes: &[u8]) -> u64 {
    fnv1a_step(0xcbf29ce484222325, bytes)
}

fn fnv1a_step(mut hash: u64, bytes: &[u8]) -> u64 {
    for b in bytes {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// 开发源 → bundle 嵌入快照同步（替代 run-dev.sh 的 rsync 段）。
/// build.rs 的 cwd = crate 根（src-tauri/），故开发源在 ../../workflows/。
/// 发布树/沙箱里没有开发源时整体跳过——bundle 已是 commit 的快照。
fn sync_workflow_bundle() {
    let data_src = Path::new("../../workflows/sansheng-liubu");
    let engine_src = Path::new("../../workflows/_engine/scripts");
    let dst = Path::new("resources/common/bundle/workflow/sansheng-liubu");
    if !data_src.is_dir() || !dst.is_dir() {
        return;
    }
    // 1. 数据（roles/registry/route_table/workflow.json）→ bundle 根
    sync_tree(data_src, dst);
    // 2. 引擎脚本 → bundle/scripts/（排 test_*，与 scheduler.py 同根路径假设对齐）
    let scripts_dst = dst.join("scripts");
    let _ = std::fs::create_dir_all(&scripts_dst);
    // watch 目录本身：往 _engine/scripts 新增一个 .py，文件级 rerun-if-changed 看不到
    // （那条路径之前不存在），但目录 mtime 会变 → 触发 build.rs 重跑、新脚本进 bundle。
    println!("cargo:rerun-if-changed={}", engine_src.display());
    if let Ok(rd) = std::fs::read_dir(engine_src) {
        for e in rd.flatten() {
            let p = e.path();
            let name = p
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            if !name.ends_with(".py") || name.starts_with("test_") {
                continue;
            }
            println!("cargo:rerun-if-changed={}", p.display());
            copy_if_changed(&p, &scripts_dst.join(&name));
        }
    }
}

/// 递归同步 src → dst（仅内容，排除 VCS/缓存/env）。
/// 删除不传播：从 src 删的文件留在 dst 快照里继续被嵌入（与原 rsync 无 --delete 一致）。
/// 这里不能加 prune——dst 还含另一步从 _engine 灌的 scripts/（src 里没有），prune 会误删它；
/// 单文件下线靠手工清 bundle。整目录下线见 build.rs 顶部 retired-workflow 注释。
fn sync_tree(src: &Path, dst: &Path) {
    // watch 目录本身：往 src 新增文件（如新角色 .md），文件级 rerun-if-changed 看不到那条
    // 尚不存在的路径，但目录 mtime 变 → 触发 build.rs 重跑、新文件进 bundle。递归覆盖每层子目录。
    println!("cargo:rerun-if-changed={}", src.display());
    let Ok(rd) = std::fs::read_dir(src) else {
        return;
    };
    for e in rd.flatten() {
        let sp = e.path();
        let name = sp
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        if name == ".git"
            || name == ".gitignore"
            || name == "__pycache__"
            || name.ends_with(".env")
            || name.ends_with(".pyc")
        {
            continue;
        }
        let dp = dst.join(&name);
        if sp.is_dir() {
            let _ = std::fs::create_dir_all(&dp);
            sync_tree(&sp, &dp);
        } else {
            println!("cargo:rerun-if-changed={}", sp.display());
            copy_if_changed(&sp, &dp);
        }
    }
}

/// 内容比较拷贝：相同则不写——避免无谓 git dirty / mtime 抖动 / bundle 重解包。
fn copy_if_changed(src: &Path, dst: &Path) {
    let Ok(new_bytes) = std::fs::read(src) else {
        return;
    };
    if let Ok(old_bytes) = std::fs::read(dst) {
        if old_bytes == new_bytes {
            return;
        }
    }
    let _ = std::fs::write(dst, new_bytes);
}
