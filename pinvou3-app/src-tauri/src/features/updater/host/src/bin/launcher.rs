// architecture-guard: allow-target-cfg -- Only standalone MSVC binaries require static CRT before entry; placing this compile guard in the shared library would also reject the GUI dependency. Actual per-bin dynamic-CRT compile-refusal controls cover this policy.
//! Fixed standalone host role; application entrypoints are separate.
// A role must enter its own prerequisite/maintenance checks without a GUI VC runtime.
// Build this leaf with an explicit MSVC target and +crt-static for ALL target crates.
#[cfg(all(windows, target_env = "msvc", not(target_feature = "crt-static")))]
compile_error!("HOST_STANDALONE_STATIC_CRT_REQUIRED");

fn main() -> std::process::ExitCode {
    pinvou_upgrade_host::launcher_main(std::env::args_os().skip(1))
}
