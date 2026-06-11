use std::path::{Path, PathBuf};

fn main() {
    // 算 bundle 资源的内容 hash，注入到 BUNDLE_INSTRUCTIONS_HASH 环境变量
    // 让 bundle.rs::BUNDLE_VERSION 自动包含 hash —— 文件变化时 cargo build 重跑
    // build.rs，hash 跟着变，ensure_extracted 自动覆写 disk 的 bundle 文件。
    //
    // 纳入哈希的资源（任一变化都要触发老安装重新解包）：
    //   - instructions.md：system prompt 模板
    //   - deny_sensitive_paths.sh：敏感目录/命令硬拦截 hook（只改它也要落盘）
    let mut hashed = Vec::new();
    for f in [
        "resources/bundle/instructions.md",
        "resources/bundle/deny_sensitive_paths.sh",
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
    let sansheng_workflow_hash = hash_dir(Path::new("resources/bundle/workflow/sansheng-liubu"));
    println!("cargo:rustc-env=BUNDLE_WORKFLOW_HASH_SANSHENG={sansheng_workflow_hash:016x}");
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
