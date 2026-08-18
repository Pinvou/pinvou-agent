fn main() {
    // Bundle system prompt and security hook changes must invalidate the
    // extracted immutable bundle on existing installations.
    let mut hashed = Vec::new();
    for file in [
        "resources/common/bundle/instructions-shared.md",
        "resources/common/bundle/instructions-work.md",
        "resources/common/bundle/instructions-code.md",
        "resources/common/bundle/deny_sensitive_paths.sh",
    ] {
        println!("cargo:rerun-if-changed={file}");
        hashed.extend(std::fs::read(file).unwrap_or_else(|_| panic!("{file} must exist")));
    }
    println!(
        "cargo:rustc-env=BUNDLE_INSTRUCTIONS_HASH={:016x}",
        fnv1a_64(&hashed)
    );
    tauri_build::build();
}

fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}
