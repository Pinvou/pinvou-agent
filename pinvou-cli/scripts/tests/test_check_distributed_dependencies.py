import importlib.util
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / "check_distributed_dependencies.py"
SPEC = importlib.util.spec_from_file_location("distributed_dependency_guard", SCRIPT)
guard = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = guard
SPEC.loader.exec_module(guard)


def package(name, manifest_path=None):
    package_id = f"path+file:///repo/pinvou-cli/crates/{name}#{name}@0.1.0"
    return {
        "id": package_id,
        "name": name,
        "manifest_path": manifest_path
        or f"/repo/pinvou-cli/crates/{name}/Cargo.toml",
    }


def dependency(name, kind=None, target=None):
    return {
        "name": name,
        "pkg": f"path+file:///repo/pinvou-cli/crates/{name}#{name}@0.1.0",
        "dep_kinds": [{"kind": kind, "target": target}],
    }


def metadata(packages, nodes):
    return {"packages": packages, "resolve": {"nodes": nodes}}


class DistributedDependencyGuardTests(unittest.TestCase):
    root = "pinvou-node"

    def violations(self, graph):
        return guard.find_violations(graph, [self.root], Path("/repo"))

    def test_clean_closure_ignores_unreachable_legacy_packages(self):
        root = package(self.root)
        protocol = package("pinvou-protocol")
        legacy = package(
            "pinvou-product-backend",
            "/repo/pinvou-cli/crates/pinvou-product-backend/Cargo.toml",
        )
        graph = metadata(
            [root, protocol, legacy],
            [
                {"id": root["id"], "features": [], "deps": [dependency("pinvou-protocol")]},
                {"id": protocol["id"], "features": [], "deps": []},
                {"id": legacy["id"], "features": [], "deps": []},
            ],
        )
        self.assertEqual([], self.violations(graph))

    def test_metadata_json_is_decoded_as_utf8_independent_of_locale(self):
        raw = '{"workspace_root":"/仓库","packages":[],"resolve":{"nodes":[]}}'.encode(
            "utf-8"
        )
        self.assertEqual("/仓库", guard.decode_metadata(raw)["workspace_root"])

    def test_rejects_direct_forbidden_dependency(self):
        root = package(self.root)
        tauri = package("tauri")
        graph = metadata(
            [root, tauri],
            [
                {"id": root["id"], "features": [], "deps": [dependency("tauri")]},
                {"id": tauri["id"], "features": [], "deps": []},
            ],
        )
        self.assertIn("tauri", "\n".join(self.violations(graph)))

    def test_rejects_forbidden_crate_name_with_underscore_separator(self):
        root = package(self.root)
        codewhale = package("codewhale_core")
        graph = metadata(
            [root, codewhale],
            [
                {"id": root["id"], "features": [], "deps": [dependency("codewhale_core")]},
                {"id": codewhale["id"], "features": [], "deps": []},
            ],
        )
        self.assertIn("codewhale_core", "\n".join(self.violations(graph)))

    def test_metadata_command_is_locked_and_selects_only_distributed_feature(self):
        command = guard.metadata_command(Path("/repo/pinvou-cli/Cargo.toml"))
        self.assertIn("--locked", command)
        self.assertEqual("pinvou-cli/distributed", command[command.index("--features") + 1])

    def test_rejects_transitive_forbidden_dependency(self):
        root = package(self.root)
        bridge = package("bridge")
        codewhale = package("codewhale-core")
        graph = metadata(
            [root, bridge, codewhale],
            [
                {"id": root["id"], "features": [], "deps": [dependency("bridge")]},
                {"id": bridge["id"], "features": [], "deps": [dependency("codewhale-core")]},
                {"id": codewhale["id"], "features": [], "deps": []},
            ],
        )
        self.assertIn("codewhale-core", "\n".join(self.violations(graph)))

    def test_rejects_build_dependency(self):
        root = package(self.root)
        backend = package("pinvou-product-backend")
        graph = metadata(
            [root, backend],
            [
                {
                    "id": root["id"],
                    "features": [],
                    "deps": [dependency("pinvou-product-backend", kind="build")],
                },
                {"id": backend["id"], "features": [], "deps": []},
            ],
        )
        self.assertIn("pinvou-product-backend", "\n".join(self.violations(graph)))

    def test_rejects_target_specific_dependency(self):
        root = package(self.root)
        tauri_runtime = package("tauri-runtime")
        graph = metadata(
            [root, tauri_runtime],
            [
                {
                    "id": root["id"],
                    "features": [],
                    "deps": [dependency("tauri-runtime", target="cfg(windows)")],
                },
                {"id": tauri_runtime["id"], "features": [], "deps": []},
            ],
        )
        self.assertIn("tauri-runtime", "\n".join(self.violations(graph)))

    def test_rejects_path_dependency_even_when_aliased(self):
        root = package(self.root)
        aliased = package("safe-alias", "/repo/pinvou3-app/src-tauri/Cargo.toml")
        graph = metadata(
            [root, aliased],
            [
                {
                    "id": root["id"],
                    "features": [],
                    "deps": [{
                        "name": "renamed_dependency",
                        "pkg": aliased["id"],
                        "dep_kinds": [{"kind": None, "target": None}],
                    }],
                },
                {"id": aliased["id"], "features": [], "deps": []},
            ],
        )
        self.assertIn("pinvou3-app", "\n".join(self.violations(graph)))

    def test_rejects_activated_legacy_feature_on_formal_root(self):
        root = package(self.root)
        graph = metadata(
            [root],
            [{"id": root["id"], "features": ["product-backend"], "deps": []}],
        )
        self.assertIn("product-backend", "\n".join(self.violations(graph)))

    def test_dev_only_dependency_is_not_part_of_release_closure(self):
        root = package(self.root)
        tauri = package("tauri")
        graph = metadata(
            [root, tauri],
            [
                {"id": root["id"], "features": [], "deps": [dependency("tauri", kind="dev")]},
                {"id": tauri["id"], "features": [], "deps": []},
            ],
        )
        self.assertEqual([], self.violations(graph))

    def test_rejects_unactivated_optional_forbidden_declaration(self):
        root = package(self.root)
        root["dependencies"] = [{
            "name": "tauri_plugin_shell",
            "kind": None,
            "optional": True,
            "path": None,
            "target": "cfg(windows)",
        }]
        graph = metadata(
            [root],
            [{"id": root["id"], "features": [], "deps": []}],
        )
        self.assertIn("tauri_plugin_shell", "\n".join(self.violations(graph)))

    def test_rejects_unactivated_forbidden_build_declaration(self):
        root = package(self.root)
        root["dependencies"] = [{
            "name": "codewhale-build-support",
            "kind": "build",
            "optional": True,
            "path": None,
            "target": None,
        }]
        graph = metadata(
            [root],
            [{"id": root["id"], "features": [], "deps": []}],
        )
        self.assertIn("codewhale-build-support", "\n".join(self.violations(graph)))

    def test_rejects_unactivated_forbidden_target_declaration(self):
        root = package(self.root)
        root["dependencies"] = [{
            "name": "tauri-runtime",
            "kind": None,
            "optional": False,
            "path": None,
            "target": "cfg(target_os = \"windows\")",
        }]
        graph = metadata(
            [root],
            [{"id": root["id"], "features": [], "deps": []}],
        )
        self.assertIn("tauri-runtime", "\n".join(self.violations(graph)))

    def test_cli_may_declare_but_not_activate_legacy_optional_backend(self):
        cli = package("pinvou-cli")
        cli["dependencies"] = [{
            "name": "pinvou-product-backend",
            "kind": None,
            "optional": True,
            "path": "/repo/pinvou-cli/crates/pinvou-product-backend",
            "target": None,
        }]
        graph = metadata(
            [cli],
            [{"id": cli["id"], "features": ["distributed"], "deps": []}],
        )
        self.assertEqual(
            [], guard.find_violations(graph, ["pinvou-cli"], Path("/repo"))
        )

    def test_rejects_forbidden_native_links_declaration(self):
        root = package(self.root)
        root["links"] = "codewhale_runtime"
        graph = metadata(
            [root],
            [{"id": root["id"], "features": [], "deps": []}],
        )
        self.assertIn("links", "\n".join(self.violations(graph)))

    def test_rejects_forbidden_build_script_link_directive(self):
        with tempfile.TemporaryDirectory() as temp:
            repo = Path(temp)
            crate = repo / "pinvou-cli/crates/pinvou-node"
            crate.mkdir(parents=True)
            build_rs = crate / "build.rs"
            build_rs.write_text(
                'fn main() { println!("cargo:rustc-link-lib=codewhale_runtime"); }',
                encoding="utf-8",
            )
            root = package(self.root, str(crate / "Cargo.toml"))
            root["targets"] = [{"kind": ["custom-build"], "src_path": str(build_rs)}]
            graph = metadata(
                [root],
                [{"id": root["id"], "features": [], "deps": []}],
            )
            violations = guard.find_violations(graph, [self.root], repo)
            self.assertIn("rustc-link-lib", "\n".join(violations))

    def test_rejects_forbidden_link_attribute(self):
        with tempfile.TemporaryDirectory() as temp:
            repo = Path(temp)
            crate = repo / "pinvou-cli/crates/pinvou-node"
            source = crate / "src/lib.rs"
            source.parent.mkdir(parents=True)
            source.write_text('#[link(name = "tauri_runtime")] extern "C" {}', encoding="utf-8")
            root = package(self.root, str(crate / "Cargo.toml"))
            root["targets"] = [{"kind": ["lib"], "src_path": str(source)}]
            graph = metadata(
                [root],
                [{"id": root["id"], "features": [], "deps": []}],
            )
            violations = guard.find_violations(graph, [self.root], repo)
            self.assertIn("#[link]", "\n".join(violations))

    def test_rejects_forbidden_dynamic_library_load(self):
        with tempfile.TemporaryDirectory() as temp:
            repo = Path(temp)
            crate = repo / "pinvou-cli/crates/pinvou-node"
            source = crate / "src/lib.rs"
            source.parent.mkdir(parents=True)
            source.write_text(
                'fn load() { unsafe { Library::new("codewhale_runtime.dll") }; }',
                encoding="utf-8",
            )
            root = package(self.root, str(crate / "Cargo.toml"))
            graph = metadata(
                [root],
                [{"id": root["id"], "features": [], "deps": []}],
            )
            violations = guard.find_violations(graph, [self.root], repo)
            self.assertIn("动态加载", "\n".join(violations))


if __name__ == "__main__":
    unittest.main()
