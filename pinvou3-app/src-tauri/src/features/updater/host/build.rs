//! Embed independently supplied PUBLIC build inputs; never read runtime trust configuration.
use std::{env, fs, path::PathBuf};
#[path = "src/build_input.rs"]
mod build_input;
#[path = "src/build_toolchain.rs"]
mod build_toolchain;

fn main() {
    // These standalone MSVC roles must restrict static imports before Rust main runs.
    // Runtime SetDefaultDllDirectories separately protects later dynamic loads.
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        for name in ["pinvou-upgrade-helper", "pinvou-upgrade-launcher"] {
            println!("cargo:rustc-link-arg-bin={name}=/DEPENDENTLOADFLAG:0x800");
        }
    }
    println!("cargo:rerun-if-env-changed=PINVOU_HOST_BOOTSTRAP_SEED");
    println!("cargo:rerun-if-env-changed=PINVOU_HOST_INITIAL_ROOT");
    let seed = env::var_os("PINVOU_HOST_BOOTSTRAP_SEED");
    let root = env::var_os("PINVOU_HOST_INITIAL_ROOT");
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo OUT_DIR"));
    let load = |value: std::ffi::OsString| {
        let path = PathBuf::from(value);
        let bytes = build_input::read_public_input(&path).expect("HOST_BUILD_INPUT_UNAVAILABLE");
        // Cargo needs the actual dependency path; only public trust material is allowed here.
        println!("cargo:rerun-if-changed={}", path.display());
        bytes
    };
    let (seed, root) = match (seed, root) {
        (None, None) => (Vec::new(), Vec::new()),
        (Some(seed), Some(root)) => (load(seed), load(root)),
        _ => panic!("HOST_BUILD_INPUT_PAIR_REQUIRED"),
    };
    build_input::validate_profile_mode(
        &seed,
        env::var_os("CARGO_FEATURE_NATIVE_ACCEPTANCE_HOOKS").is_some(),
    )
    .unwrap_or_else(|code| panic!("{code}"));
    let minimum = if seed.is_empty() {
        "0.0.0.0".to_owned()
    } else if env::var("TARGET").as_deref() == Ok("x86_64-pc-windows-msvc") {
        for flags in ["RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS"] {
            if env::var(flags).is_ok_and(|value| value.contains("linker=")) {
                panic!("HOST_BUILD_LINKER_MISMATCH");
            }
            println!("cargo:rerun-if-env-changed={flags}");
        }
        let tool = find_msvc_tools::find_tool("x86_64-pc-windows-msvc", "link.exe")
            .expect("HOST_BUILD_MSVC_TOOLCHAIN_REQUIRED");
        if let Some(selected) = env::var_os("RUSTC_LINKER") {
            let selected = fs::canonicalize(selected).expect("HOST_BUILD_LINKER_UNAVAILABLE");
            let actual = fs::canonicalize(tool.path()).expect("HOST_BUILD_LINKER_UNAVAILABLE");
            if selected != actual {
                panic!("HOST_BUILD_LINKER_MISMATCH");
            }
        }
        build_toolchain::minimum_from_msvc_linker(tool.path())
            .expect("HOST_BUILD_MSVC_VERSION_UNAVAILABLE")
    } else {
        "0.0.0.0".to_owned()
    };
    println!("cargo:rustc-env=PINVOU_HOST_MINIMUM_VC_RUNTIME={minimum}");
    for name in [
        "RUSTC_LINKER",
        "VCINSTALLDIR",
        "VCToolsInstallDir",
        "VCToolsVersion",
        "VSINSTALLDIR",
        "PATH",
    ] {
        println!("cargo:rerun-if-env-changed={name}");
    }
    fs::write(output.join("bootstrap-seed.json"), seed).expect("HOST_BUILD_OUTPUT_UNAVAILABLE");
    fs::write(output.join("initial-root.json"), root).expect("HOST_BUILD_OUTPUT_UNAVAILABLE");
}
