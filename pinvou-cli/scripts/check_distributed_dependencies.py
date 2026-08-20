#!/usr/bin/env python3
"""Fail when the distributed release closure reaches Desktop/legacy crates."""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from collections import deque
from pathlib import Path


DEFAULT_ROOTS = (
    "pinvou-cli",
    "pinvou-controller",
    "pinvou-node",
    "pinvou-protocol",
    "pinvou-seglog",
    "pinvou-runtime-api",
    "pinvou-agent-adapter-codex",
)


def _is_forbidden_name(name: str) -> bool:
    lowered = name.casefold().replace("_", "-")
    return (
        lowered == "pinvou-product-backend"
        or lowered == "tauri"
        or lowered.startswith("tauri-")
        or lowered == "codewhale"
        or lowered.startswith("codewhale-")
    )


def _has_forbidden_brand(text: str) -> bool:
    normalized = text.casefold().replace("_", "-")
    return any(
        brand in normalized
        for brand in ("pinvou-product-backend", "pinvou3-app", "tauri", "codewhale")
    )


def _is_within(path: Path, directory: Path) -> bool:
    try:
        path.resolve(strict=False).relative_to(directory.resolve(strict=False))
        return True
    except ValueError:
        return False


def _forbidden_manifest_reason(manifest_path: str, repo_root: Path) -> str | None:
    manifest = Path(manifest_path)
    for directory_name in ("pinvou3-app", "CodeWhale"):
        if _is_within(manifest, repo_root / directory_name):
            return f"manifest 位于 {directory_name}/"
    return None


def _release_dependency_ids(node: dict) -> list[str]:
    dependency_ids = []
    for dependency in node.get("deps", []):
        kinds = dependency.get("dep_kinds", [])
        # Older metadata may omit dep_kinds; conservatively treat that as normal.
        if not kinds or any(kind.get("kind") in (None, "normal", "build") for kind in kinds):
            dependency_ids.append(dependency["pkg"])
    return dependency_ids


def _declared_package_ids(
    packages: dict[str, dict], root_ids: list[str], repo_root: Path
) -> tuple[set[str], list[str]]:
    """Follow every local normal/build declaration, including optional/target edges."""
    by_directory = {
        Path(package["manifest_path"]).resolve(strict=False).parent: package_id
        for package_id, package in packages.items()
    }
    queue = deque(root_ids)
    visited = set()
    violations = []
    while queue:
        package_id = queue.popleft()
        if package_id in visited:
            continue
        visited.add(package_id)
        package = packages[package_id]
        owner = package["name"]
        for dependency in package.get("dependencies", []):
            if dependency.get("kind") == "dev":
                continue
            dependency_name = dependency["name"]
            legacy_cli_exception = (
                owner == "pinvou-cli"
                and dependency_name == "pinvou-product-backend"
                and dependency.get("optional") is True
            )
            if _is_forbidden_name(dependency_name) and not legacy_cli_exception:
                violations.append(
                    f"{owner}: 声明了违禁 normal/build 依赖 {dependency_name}"
                )
            dependency_path = dependency.get("path")
            if not dependency_path:
                continue
            path = Path(dependency_path).resolve(strict=False)
            path_reason = _forbidden_manifest_reason(str(path / "Cargo.toml"), repo_root)
            if path_reason and not legacy_cli_exception:
                violations.append(f"{owner} -> {dependency_name}: {path_reason}")
            local_id = by_directory.get(path)
            if local_id is not None and not legacy_cli_exception:
                queue.append(local_id)
    return visited, violations


def _source_link_violations(package: dict) -> list[str]:
    violations = []
    links = package.get("links")
    if links and _has_forbidden_brand(links):
        violations.append(f"{package['name']}: links={links!r} 引用违禁 native library")

    crate_dir = Path(package["manifest_path"]).parent
    candidates: dict[Path, bool] = {}
    build_script = crate_dir / "build.rs"
    if build_script.is_file():
        candidates[build_script] = True
    source_dir = crate_dir / "src"
    if source_dir.is_dir():
        for source in sorted(source_dir.rglob("*.rs")):
            candidates.setdefault(source, False)
    for target in package.get("targets", []):
        source_path = target.get("src_path")
        if not source_path:
            continue
        source = Path(source_path)
        if source.is_file():
            is_custom_build = "custom-build" in target.get("kind", [])
            candidates[source] = candidates.get(source, False) or is_custom_build
    for source, is_custom_build in candidates.items():
        try:
            content = source.read_text(encoding="utf-8")
        except (OSError, UnicodeError) as error:
            violations.append(f"{package['name']}: 无法检查 {source}: {error}")
            continue
        if is_custom_build:
            for line in content.splitlines():
                if "rustc-link-lib" in line and _has_forbidden_brand(line):
                    violations.append(
                        f"{package['name']}: {source} 的 rustc-link-lib 引用违禁库"
                    )
        link_attributes = re.finditer(
            r"#\s*\[\s*link\s*\((?P<body>[^]]*)\)\s*\]", content, re.DOTALL
        )
        if any(_has_forbidden_brand(match.group("body")) for match in link_attributes):
            violations.append(f"{package['name']}: {source} 的 #[link] 引用违禁库")
        dynamic_loads = re.finditer(
            r"(?:Library::new|LoadLibrary\w*|dlopen)\s*\((?P<body>[^;\n]*)\)",
            content,
        )
        if any(_has_forbidden_brand(match.group("body")) for match in dynamic_loads):
            violations.append(f"{package['name']}: {source} 动态加载违禁库")
    return violations


def find_violations(metadata: dict, root_names: list[str], repo_root: Path) -> list[str]:
    """Inspect only the resolved normal/build closure rooted at release packages."""
    packages = {package["id"]: package for package in metadata.get("packages", [])}
    nodes = {
        node["id"]: node
        for node in (metadata.get("resolve") or {}).get("nodes", [])
    }
    ids_by_name: dict[str, list[str]] = {}
    for package_id, package in packages.items():
        ids_by_name.setdefault(package["name"], []).append(package_id)

    violations = []
    root_ids = []
    for root_name in root_names:
        matches = ids_by_name.get(root_name, [])
        if len(matches) != 1:
            violations.append(
                f"正式 root {root_name!r} 解析到 {len(matches)} 个 package，预期恰好 1 个"
            )
        else:
            root_ids.append(matches[0])
    if violations:
        return violations

    declared_ids, declared_violations = _declared_package_ids(
        packages, root_ids, repo_root
    )
    violations.extend(declared_violations)
    for package_id in declared_ids:
        violations.extend(_source_link_violations(packages[package_id]))

    queue = deque((package_id, [packages[package_id]["name"]]) for package_id in root_ids)
    visited = set()
    while queue:
        package_id, chain = queue.popleft()
        if package_id in visited:
            continue
        visited.add(package_id)
        package = packages.get(package_id)
        node = nodes.get(package_id)
        if package is None or node is None:
            violations.append(f"resolved graph 缺少 package/node: {' -> '.join(chain)}")
            continue

        reasons = []
        if _is_forbidden_name(package["name"]):
            reasons.append(f"违禁 crate 名 {package['name']}")
        manifest_reason = _forbidden_manifest_reason(package["manifest_path"], repo_root)
        if manifest_reason:
            reasons.append(manifest_reason)
        if "product-backend" in node.get("features", []):
            reasons.append("启用了 legacy feature product-backend")
        if reasons:
            violations.append(f"{' -> '.join(chain)}: {', '.join(reasons)}")

        for dependency_id in _release_dependency_ids(node):
            dependency = packages.get(dependency_id)
            dependency_name = dependency["name"] if dependency else dependency_id
            queue.append((dependency_id, [*chain, dependency_name]))

    return sorted(set(violations))


def decode_metadata(raw: bytes) -> dict:
    return json.loads(raw.decode("utf-8"))


def metadata_command(manifest_path: Path) -> list[str]:
    return [
        "cargo",
        "metadata",
        "--locked",
        "--format-version",
        "1",
        "--manifest-path",
        str(manifest_path),
        "--no-default-features",
        "--features",
        "pinvou-cli/distributed",
    ]


def _run_metadata(manifest_path: Path) -> dict:
    command = metadata_command(manifest_path)
    completed = subprocess.run(command, check=True, capture_output=True)
    return decode_metadata(completed.stdout)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    default_repo = Path(__file__).resolve().parents[2]
    parser.add_argument("--repo-root", type=Path, default=default_repo)
    parser.add_argument(
        "--manifest-path",
        type=Path,
        default=default_repo / "pinvou-cli" / "Cargo.toml",
    )
    parser.add_argument("--root", action="append", dest="roots")
    parser.add_argument(
        "--metadata-json",
        type=Path,
        help="读取既有 cargo metadata JSON；缺省时执行 cargo metadata",
    )
    args = parser.parse_args(argv)

    graph = (
        json.loads(args.metadata_json.read_text(encoding="utf-8"))
        if args.metadata_json
        else _run_metadata(args.manifest_path)
    )
    violations = find_violations(graph, args.roots or list(DEFAULT_ROOTS), args.repo_root)
    if violations:
        print("distributed resolved dependency boundary failed:", file=sys.stderr)
        for violation in violations:
            print(f"- {violation}", file=sys.stderr)
        return 1
    print("distributed resolved dependency boundary passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
