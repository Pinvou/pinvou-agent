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

    tauri_build::build();
}

/// 简易 FNV-1a 64 位 hash —— 不需要 cryptographic 强度，只要内容变化时 hash 变即可。
fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in bytes {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}
