fn main() {
    // 算 instructions.md 的内容 hash，注入到 BUNDLE_INSTRUCTIONS_HASH 环境变量
    // 让 bundle.rs::BUNDLE_VERSION 自动包含 hash —— 文件变化时 cargo build 重跑
    // build.rs，hash 跟着变，ensure_extracted 自动覆写 disk 的 instructions.md。
    println!("cargo:rerun-if-changed=resources/bundle/instructions.md");
    let content = std::fs::read("resources/bundle/instructions.md")
        .expect("bundle/instructions.md must exist");
    let hash = fnv1a_64(&content);
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
