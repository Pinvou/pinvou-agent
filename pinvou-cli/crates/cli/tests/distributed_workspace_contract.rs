use std::path::PathBuf;
use std::process::Command;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .expect("cli crate must live below the workspace root")
        .to_path_buf()
}

fn workspace_metadata() -> serde_json::Value {
    let output = Command::new(env!("CARGO"))
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(workspace_root())
        .output()
        .expect("cargo metadata must run");
    assert!(output.status.success());
    serde_json::from_slice(&output.stdout).expect("metadata must be JSON")
}

#[test]
fn distributed_workspace_has_the_stage_one_crate_and_binary_identities() {
    let metadata = workspace_metadata();
    let packages = metadata["packages"]
        .as_array()
        .expect("metadata packages must be an array");

    for name in [
        "pinvou-controller",
        "pinvou-node",
        "pinvou-protocol",
        "pinvou-seglog",
        "pinvou-runtime-api",
        "pinvou-agent-adapter-codex",
    ] {
        assert!(
            packages.iter().any(|package| package["name"] == name),
            "missing workspace package {name}"
        );
    }

    for (package_name, binary_name) in [
        ("pinvou-controller", "pinvou-controller"),
        ("pinvou-node", "pinvou-node"),
    ] {
        let package = packages
            .iter()
            .find(|package| package["name"] == package_name)
            .expect("daemon package must exist");
        let targets = package["targets"]
            .as_array()
            .expect("targets must be an array");
        let library_name = package_name.replace('-', "_");
        assert!(targets.iter().any(|target| {
            target["name"] == library_name
                && target["kind"]
                    .as_array()
                    .is_some_and(|kinds| kinds.iter().any(|kind| kind == "lib"))
        }));
        assert!(targets.iter().any(|target| {
            target["name"] == binary_name
                && target["kind"]
                    .as_array()
                    .is_some_and(|kinds| kinds.iter().any(|kind| kind == "bin"))
        }));
    }
}

#[test]
fn distributed_crates_keep_the_minimal_internal_dependency_graph() {
    let metadata = workspace_metadata();
    let packages = metadata["packages"].as_array().unwrap();
    let expected = [
        ("pinvou-protocol", &[][..]),
        ("pinvou-seglog", &[][..]),
        ("pinvou-runtime-api", &["pinvou-protocol"][..]),
        ("pinvou-agent-adapter-codex", &["pinvou-runtime-api"][..]),
        (
            "pinvou-controller",
            &["pinvou-protocol", "pinvou-seglog"][..],
        ),
        (
            "pinvou-node",
            &["pinvou-protocol", "pinvou-runtime-api", "pinvou-seglog"][..],
        ),
    ];

    for (package_name, expected_dependencies) in expected {
        let package = packages
            .iter()
            .find(|package| package["name"] == package_name)
            .unwrap();
        let mut dependencies = package["dependencies"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|dependency| dependency["path"].is_string())
            .map(|dependency| dependency["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        dependencies.sort_unstable();
        assert_eq!(dependencies, expected_dependencies, "{package_name}");
    }
}

#[test]
fn distributed_cli_compiles_without_default_or_product_backend_features() {
    let target_dir = std::env::temp_dir().join(format!(
        "pinvou-distributed-contract-{}",
        std::process::id()
    ));
    let output = Command::new(env!("CARGO"))
        .args([
            "check",
            "--locked",
            "-p",
            "pinvou-cli",
            "--no-default-features",
            "--features",
            "distributed",
        ])
        .env("CARGO_TARGET_DIR", &target_dir)
        .current_dir(workspace_root())
        .output()
        .expect("cargo check must run");

    let _ = std::fs::remove_dir_all(target_dir);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let tree = Command::new(env!("CARGO"))
        .args([
            "tree",
            "--locked",
            "-p",
            "pinvou-cli",
            "--no-default-features",
            "--features",
            "distributed",
            "--prefix",
            "none",
        ])
        .current_dir(workspace_root())
        .output()
        .expect("cargo tree must run");
    assert!(tree.status.success());
    let resolved = String::from_utf8(tree.stdout).expect("cargo tree output must be UTF-8");
    for forbidden in [
        "pinvou-product-backend",
        "pinvou3-tauri",
        "tauri",
        "codewhale",
    ] {
        assert!(
            !resolved.contains(forbidden),
            "resolved forbidden {forbidden}"
        );
    }
}

#[test]
fn legacy_default_feature_remains_product_backend_only() {
    let manifest = std::fs::read_to_string(workspace_root().join("crates/cli/Cargo.toml"))
        .expect("cli manifest must be readable");
    assert!(manifest.contains("default = [\"product-backend\"]"));
    assert!(manifest.contains("distributed = ["));
}
