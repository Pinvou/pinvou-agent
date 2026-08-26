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

    // tauri-build links OUT_DIR/resource.lib only into application binaries.
    // A library unit-test harness is not a Cargo `test` target, so
    // rustc-link-arg-tests does not reach it. Wrap the generated COFF resource
    // object in an archive that a cfg(test) link attribute can include without
    // adding a duplicate resource to the application binary.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        let out_dir = std::path::PathBuf::from(
            std::env::var_os("OUT_DIR").expect("Cargo must provide OUT_DIR to build.rs"),
        );
        let generated_resource = out_dir.join("resource.lib");
        let test_resource_archive = out_dir.join("pinvou3_lib_test_resource.lib");
        let _ = std::fs::remove_file(&test_resource_archive);

        let target = std::env::var("TARGET").expect("Cargo must provide TARGET to build.rs");
        let librarian = cc::windows_registry::find_tool(&target, "lib.exe")
            .expect("MSVC lib.exe is required to archive the Windows test manifest resource");
        let machine = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
            Ok("x86_64") => "X64",
            Ok("x86") => "X86",
            Ok("aarch64") => "ARM64",
            Ok("arm") => "ARM",
            other => panic!("Unsupported MSVC target architecture for test resources: {other:?}"),
        };
        let output = librarian
            .to_command()
            .arg("/NOLOGO")
            .arg(format!("/MACHINE:{machine}"))
            .arg(format!("/OUT:{}", test_resource_archive.display()))
            .arg(&generated_resource)
            .output()
            .expect("Failed to run MSVC lib.exe for the Windows test resource");
        if !output.status.success() {
            panic!(
                "Failed to archive the Windows test manifest resource: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        println!("cargo:rustc-link-search=native={}", out_dir.display());
    }
}

fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}
