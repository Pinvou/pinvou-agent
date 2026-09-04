#!/usr/bin/env python3
"""Enforce pinvou3 module and platform boundaries without third-party packages.

The checked-in baseline records existing architecture debt. A check passes when
the debt stays unchanged or decreases; new violation kinds, new files/edges, and
increased counts fail. This makes the guard useful before the migration is fully
complete without normalising new debt.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from collections import Counter, defaultdict
from pathlib import Path
from typing import Iterable


SCHEMA_VERSION = 1
DEFAULT_BASELINE = Path("scripts/architecture-baseline.json")
FRONTEND_SUFFIXES = {".html", ".js", ".jsx", ".mjs", ".cjs", ".ts", ".tsx"}


def normalize(path: Path, root: Path) -> str:
    return path.resolve().relative_to(root.resolve()).as_posix()


def source_files(root: Path, directory: str, suffixes: set[str]) -> Iterable[Path]:
    base = root / directory
    if not base.exists():
        return []
    return (
        path
        for path in base.rglob("*")
        if path.is_file() and path.suffix.lower() in suffixes
    )


def read_text(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def git_head(root: Path) -> str:
    try:
        return subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=root, text=True, stderr=subprocess.DEVNULL
        ).strip()
    except (OSError, subprocess.CalledProcessError):
        return "unknown"


def baseline_from_git(root: Path, ref: str, relative_path: Path) -> dict | None:
    git_path = relative_path.as_posix()
    try:
        subprocess.check_call(
            ["git", "rev-parse", "--verify", f"{ref}^{{commit}}"],
            cwd=root,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
    except (OSError, subprocess.CalledProcessError) as error:
        raise ValueError(f"Git base ref is unavailable: {ref}") from error
    try:
        content = subprocess.check_output(
            ["git", "show", f"{ref}:{git_path}"],
            cwd=root,
            text=True,
            stderr=subprocess.DEVNULL,
        )
    except (OSError, subprocess.CalledProcessError):
        return None
    try:
        return json.loads(content)
    except json.JSONDecodeError as error:
        raise ValueError(f"invalid baseline at {ref}:{git_path}: {error}") from error


def rust_aliases(root: Path) -> dict[str, tuple[str, str]]:
    """Return crate-root compatibility aliases as alias -> (layer, module)."""
    lib_rs = root / "pinvou3-app/src-tauri/src/lib.rs"
    if not lib_rs.exists():
        return {}
    text = read_text(lib_rs)
    pattern = re.compile(
        r'#\s*\[\s*path\s*=\s*"(?P<path>[^"]+)"\s*\]\s*'
        r'(?:pub(?:\([^)]*\))?\s+)?mod\s+(?P<alias>[A-Za-z_][A-Za-z0-9_]*)\s*;'
    )
    aliases: dict[str, tuple[str, str]] = {}
    for match in pattern.finditer(text):
        parts = Path(match.group("path")).as_posix().split("/")
        if len(parts) >= 2 and parts[0] in {"app", "features", "platform", "core"}:
            aliases[match.group("alias")] = (parts[0], Path(parts[1]).stem)
    return aliases


def crate_references(text: str, aliases: dict[str, tuple[str, str]]) -> list[tuple[str, str]]:
    refs: list[tuple[str, str]] = []
    direct = re.compile(
        r"\bcrate\s*::\s*(app|features|platform|core)\s*::\s*"
        r"([A-Za-z_][A-Za-z0-9_]*)"
    )
    direct_spans: list[tuple[int, int]] = []
    for match in direct.finditer(text):
        refs.append((match.group(1), match.group(2)))
        direct_spans.append(match.span())

    root_ref = re.compile(r"\bcrate\s*::\s*([A-Za-z_][A-Za-z0-9_]*)")
    for match in root_ref.finditer(text):
        if any(start <= match.start() < end for start, end in direct_spans):
            continue
        target = aliases.get(match.group(1))
        if target:
            refs.append(target)

    grouped_start = re.compile(r"\bcrate\s*::\s*\{")
    for match in grouped_start.finditer(text):
        cursor = match.end()
        depth = 1
        while cursor < len(text) and depth:
            if text[cursor] == "{":
                depth += 1
            elif text[cursor] == "}":
                depth -= 1
            cursor += 1
        if depth:
            continue
        content = text[match.end() : cursor - 1]
        items: list[str] = []
        item_start = 0
        item_depth = 0
        for index, character in enumerate(content):
            if character == "{":
                item_depth += 1
            elif character == "}":
                item_depth -= 1
            elif character == "," and item_depth == 0:
                items.append(content[item_start:index])
                item_start = index + 1
        items.append(content[item_start:])
        for item in items:
            identifiers = re.findall(r"[A-Za-z_][A-Za-z0-9_]*", item)
            if not identifiers:
                continue
            first = identifiers[0]
            if first in {"app", "features", "platform", "core"} and len(identifiers) > 1:
                refs.append((first, identifiers[1]))
            elif first in aliases:
                refs.append(aliases[first])
    return refs


def frontend_import_target(specifier: str, source: Path, src_root: Path) -> Path | None:
    if specifier.startswith("."):
        return (source.parent / specifier).resolve()
    if specifier.startswith("@/"):
        return (src_root / specifier[2:]).resolve()
    if specifier.startswith("src/"):
        return (src_root.parent / specifier).resolve()
    return None


def scan_frontend(root: Path) -> dict[str, Counter[str]]:
    rules: dict[str, Counter[str]] = {
        "frontend_feature_imports_app": Counter(),
        "frontend_tauri_global_outside_platform": Counter(),
        "frontend_user_agent_platform_detection": Counter(),
    }
    src_root = (root / "pinvou3-app/src").resolve()
    feature_root = (src_root / "features").resolve()
    app_root = (src_root / "app").resolve()
    platform_root = (src_root / "platform").resolve()
    import_patterns = [
        re.compile(
            r"(?:^|\n)\s*(?:import|export)\s+"
            r"(?:[A-Za-z0-9_$*,{}\s]+?\s+from\s+)?[\"']([^\"']+)[\"']"
        ),
        re.compile(r"\bimport\s*\(\s*[\"']([^\"']+)[\"']\s*\)"),
    ]
    tauri_pattern = re.compile(r"\b(?:window|globalThis)\s*\.\s*__TAURI__\b")
    user_agent_pattern = re.compile(
        r"\b(?:window\s*\.\s*)?navigator\s*\.\s*(?:userAgent|userAgentData|platform)\b"
    )

    for path in source_files(root, "pinvou3-app/src", FRONTEND_SUFFIXES):
        text = read_text(path)
        relative = normalize(path, root)
        resolved = path.resolve()
        if resolved.is_relative_to(feature_root):
            for pattern in import_patterns:
                for match in pattern.finditer(text):
                    target = frontend_import_target(match.group(1), resolved, src_root)
                    if target and target.is_relative_to(app_root):
                        feature = resolved.relative_to(feature_root).parts[0]
                        rules["frontend_feature_imports_app"][f"{feature}->{match.group(1)}"] += 1
        if not resolved.is_relative_to(platform_root):
            count = len(tauri_pattern.findall(text))
            if count:
                rules["frontend_tauri_global_outside_platform"][relative] += count
        count = len(user_agent_pattern.findall(text))
        if count:
            rules["frontend_user_agent_platform_detection"][relative] += count
    return rules


def feature_name(path: Path, feature_root: Path) -> str:
    return path.resolve().relative_to(feature_root.resolve()).parts[0]


def strongly_connected_components(graph: dict[str, set[str]]) -> list[list[str]]:
    index = 0
    indices: dict[str, int] = {}
    lowlinks: dict[str, int] = {}
    stack: list[str] = []
    on_stack: set[str] = set()
    components: list[list[str]] = []

    def visit(node: str) -> None:
        nonlocal index
        indices[node] = index
        lowlinks[node] = index
        index += 1
        stack.append(node)
        on_stack.add(node)
        for target in sorted(graph.get(node, set())):
            if target not in indices:
                visit(target)
                lowlinks[node] = min(lowlinks[node], lowlinks[target])
            elif target in on_stack:
                lowlinks[node] = min(lowlinks[node], indices[target])
        if lowlinks[node] == indices[node]:
            component: list[str] = []
            while stack:
                target = stack.pop()
                on_stack.remove(target)
                component.append(target)
                if target == node:
                    break
            if len(component) > 1 or node in graph.get(node, set()):
                components.append(sorted(component))

    nodes = set(graph)
    for targets in graph.values():
        nodes.update(targets)
    for node in sorted(nodes):
        if node not in indices:
            visit(node)
    return sorted(components)


def target_cfg_allowed(relative: str) -> bool:
    # lib.rs is the composition root; platform selection is legitimate there.
    return "/platform/" in f"/{relative}" or relative.endswith("/src/lib.rs")


def platform_detail_allowed(relative: str) -> bool:
    return "/platform/" in f"/{relative}"


def file_exception_allowed(text: str, exception: str) -> bool:
    """Return whether a reasoned file-level exception appears near the header."""
    header = "\n".join(text.splitlines()[:20])
    pattern = re.compile(
        rf"(?m)^[ \t]*//[ \t]*architecture-guard:[ \t]*"
        rf"allow-{re.escape(exception)}[ \t]+--[ \t]+\S[^\r\n]*$"
    )
    return bool(pattern.search(header))


def count_platform_cfgs(text: str) -> int:
    """Count platform selectors inside cfg/cfg_attr expressions.

    Rust accepts both key/value selectors (`target_os = "windows"`,
    `target_arch = "aarch64"`, ...) and shorthand predicates (`windows`,
    `unix`).  Keep the scanner deliberately syntax-light, but balance nested
    parentheses so `all(...)`, `any(...)` and `not(...)` are covered without
    matching ordinary prose or identifiers outside cfg expressions.
    """
    invocation = re.compile(r"\b(?:cfg|cfg_attr)\s*!?\s*\(")
    target_selector = re.compile(
        r"\b(?:target_os|target_arch|target_family|target_env|target_vendor|"
        r"target_endian|target_pointer_width|target_abi)\s*="
    )
    shorthand_selector = re.compile(r"(?<![A-Za-z0-9_])(?:windows|unix)(?![A-Za-z0-9_])")
    count = 0
    for match in invocation.finditer(text):
        cursor = match.end()
        depth = 1
        while cursor < len(text) and depth:
            if text[cursor] == "(":
                depth += 1
            elif text[cursor] == ")":
                depth -= 1
            cursor += 1
        if depth:
            continue
        expression = text[match.end() : cursor - 1]
        count += len(target_selector.findall(expression))
        expression_without_strings = re.sub(r'"(?:\\.|[^"\\])*"', '""', expression)
        count += len(shorthand_selector.findall(expression_without_strings))
    return count


def scan_rust(root: Path) -> tuple[dict[str, Counter[str]], list[list[str]]]:
    rules: dict[str, Counter[str]] = {
        "rust_feature_depends_on_app": Counter(),
        "rust_platform_depends_on_upper_layer": Counter(),
        "rust_cyclic_feature_dependencies": Counter(),
        "rust_target_cfg_outside_adapter": Counter(),
        "rust_platform_details_outside_adapter": Counter(),
        "rust_tauri_commands_outside_app": Counter(),
        "rust_tauri_handler_outside_app": Counter(),
        "rust_external_group_kill_spawn": Counter(),
    }
    aliases = rust_aliases(root)
    rust_root = root / "pinvou3-app/src-tauri/src"
    feature_root = rust_root / "features"
    platform_root = rust_root / "platform"
    graph: dict[str, set[str]] = defaultdict(set)
    feature_edge_counts: Counter[tuple[str, str]] = Counter()
    tauri_command_pattern = re.compile(r"#\s*\[\s*tauri\s*::\s*command\b")
    tauri_handler_pattern = re.compile(r"generate_handler\s*!\s*\[(.*?)\]", re.DOTALL)
    # Process-group kills must be issued through kill(2) directly. Never
    # spawn an external `kill` binary: procps-ng 4.0.4 misparses the valid
    # negative pid in `kill -9 -<pgid>` as -1 (kill(-1) signals every process
    # of the user and took down whole desktop sessions; see
    # platform::process::kill_process_tree). The rule applies to every Rust
    # file: unsupported.rs also carries the live macOS kill_pid_tree, so no
    # file-level exception may waive it.
    external_group_kill_patterns = [
        re.compile(r'Command\s*::\s*new\s*\(\s*"(?:[^"]*/)?kill"'),
        re.compile(r'::\s*new\s*\(\s*"(?:[^"]*/)?kill"\s*\)'),
        re.compile(r'Path\s*::\s*new\s*\(\s*"(?:[^"]*/)?kill"'),
        re.compile(r'connector_cli_command\s*\(\s*[^,()]*,\s*"kill"'),
    ]
    platform_detail_patterns = [
        re.compile(r"\bpowershell(?:\.exe)?\b", re.IGNORECASE),
        re.compile(r"\bxdg-open\b"),
        re.compile(r"\bDBUS_SESSION_BUS_ADDRESS\b"),
        re.compile(r"\bORT_DYLIB_PATH\b"),
        re.compile(r"\bWEBKIT_(?:DISABLE|DMABUF|FORCE|WEB_RENDER)[A-Z0-9_]*\b"),
        re.compile(r"\b(?:HKEY_[A-Z_]+|Win32::System::Registry)\b"),
        re.compile(
            r'Command\s*::\s*new\s*\(\s*"(?:open|explorer(?:\.exe)?|reg(?:\.exe)?)"'
        ),
    ]

    for path in source_files(root, "pinvou3-app/src-tauri/src", {".rs"}):
        text = read_text(path)
        relative = normalize(path, root)
        resolved = path.resolve()
        references = crate_references(text, aliases)
        if resolved.is_relative_to(feature_root.resolve()):
            source_feature = feature_name(resolved, feature_root)
            for layer, target in references:
                if layer == "app":
                    rules["rust_feature_depends_on_app"][f"{source_feature}->{target}"] += 1
                elif layer == "features" and target != source_feature:
                    graph[source_feature].add(target)
                    feature_edge_counts[(source_feature, target)] += 1
        if resolved.is_relative_to(platform_root.resolve()):
            for layer, target in references:
                if layer in {"app", "features"}:
                    rules["rust_platform_depends_on_upper_layer"][
                        f"{relative}->{layer}::{target}"
                    ] += 1
        target_count = count_platform_cfgs(text)
        if (
            target_count
            and not target_cfg_allowed(relative)
            and not file_exception_allowed(text, "target-cfg")
        ):
            rules["rust_target_cfg_outside_adapter"][relative] += target_count
        if (
            not platform_detail_allowed(relative)
            and not file_exception_allowed(text, "platform-detail")
        ):
            detail_count = sum(
                len(pattern.findall(text)) for pattern in platform_detail_patterns
            )
            if detail_count:
                rules["rust_platform_details_outside_adapter"][relative] += detail_count
        command_count = len(tauri_command_pattern.findall(text))
        if command_count and "/app/commands/" not in f"/{relative}":
            rules["rust_tauri_commands_outside_app"][relative] += command_count
        external_kill_count = sum(
            len(pattern.findall(text)) for pattern in external_group_kill_patterns
        )
        if external_kill_count:
            rules["rust_external_group_kill_spawn"][relative] += external_kill_count
        for handler in tauri_handler_pattern.findall(text):
            for entry in re.findall(
                r"(?:^|,)\s*([A-Za-z_][A-Za-z0-9_:]*)\s*(?=,|$)", handler
            ):
                if not entry.startswith("commands::") and not entry.startswith(
                    "crate::app::commands::"
                ):
                    rules["rust_tauri_handler_outside_app"][f"{relative}:{entry}"] += 1
    cycles = strongly_connected_components(graph)
    for source_target, count in feature_edge_counts.items():
        source, target = source_target
        if any({source, target}.issubset(set(cycle)) for cycle in cycles):
            rules["rust_cyclic_feature_dependencies"][f"{source}->{target}"] = count
    return rules, cycles


def scan_resources(root: Path) -> dict[str, Counter[str]]:
    rules = {"common_platform_binaries": Counter()}
    common_root = root / "pinvou3-app/src-tauri/resources/common"
    if not common_root.exists():
        return rules
    for path in common_root.rglob("*"):
        if not path.is_file():
            continue
        # Generated package-manager dependencies are ignored and never committed.
        # Packaging has its own lockfile/install contract for this directory.
        if "node_modules" in path.parts:
            continue
        try:
            magic = path.read_bytes()[:4]
        except OSError:
            continue
        if magic.startswith(b"MZ") or magic == b"\x7fELF":
            rules["common_platform_binaries"][normalize(path, root)] += 1
    return rules


def current_state(root: Path) -> dict:
    frontend_rules = scan_frontend(root)
    rust_rules, cycles = scan_rust(root)
    resource_rules = scan_resources(root)
    rules = {**frontend_rules, **rust_rules, **resource_rules}
    serializable_rules = {
        name: dict(sorted(counts.items())) for name, counts in sorted(rules.items())
    }
    state = {
        "schema_version": SCHEMA_VERSION,
        "generated_from": git_head(root),
        "rules": serializable_rules,
        "rust_feature_cycles": cycles,
    }
    return state


def compare_counts(rule: str, current: dict[str, int], allowed: dict[str, int]) -> tuple[list[str], list[str]]:
    failures: list[str] = []
    progress: list[str] = []
    for key, count in sorted(current.items()):
        limit = allowed.get(key, 0)
        if count > limit:
            failures.append(f"{rule}: {key}: found {count}, baseline allows {limit}")
    for key, limit in sorted(allowed.items()):
        count = current.get(key, 0)
        if count < limit:
            progress.append(f"{rule}: {key}: reduced {limit} -> {count}")
    return failures, progress


def cycle_allowed(cycle: list[str], baseline_cycles: list[list[str]]) -> bool:
    current = set(cycle)
    if len(current) == 1:
        return any(current == set(allowed) for allowed in baseline_cycles)
    return any(current.issubset(set(allowed)) for allowed in baseline_cycles)


def compare(current: dict, baseline: dict) -> tuple[list[str], list[str]]:
    if baseline.get("schema_version") != SCHEMA_VERSION:
        return [
            f"baseline schema mismatch: expected {SCHEMA_VERSION}, "
            f"found {baseline.get('schema_version')}"
        ], []
    failures: list[str] = []
    progress: list[str] = []
    current_rules = current.get("rules", {})
    baseline_rules = baseline.get("rules", {})
    for rule in sorted(set(current_rules) | set(baseline_rules)):
        rule_failures, rule_progress = compare_counts(
            rule, current_rules.get(rule, {}), baseline_rules.get(rule, {})
        )
        failures.extend(rule_failures)
        progress.extend(rule_progress)
    current_cycles = current.get("rust_feature_cycles", [])
    baseline_cycles = baseline.get("rust_feature_cycles", [])
    current_sets = [set(cycle) for cycle in current_cycles]
    baseline_sets = [set(cycle) for cycle in baseline_cycles]
    for cycle, current_set in zip(current_cycles, current_sets):
        if any(current_set == allowed for allowed in baseline_sets):
            continue
        if len(current_set) > 1 and any(current_set < allowed for allowed in baseline_sets):
            progress.append(f"rust_feature_cycles: cycle shrank to {', '.join(cycle)}")
        else:
            failures.append(f"rust_feature_cycles: new or expanded cycle: {' -> '.join(cycle)}")
    for cycle, baseline_set in zip(baseline_cycles, baseline_sets):
        if any(baseline_set == current_set for current_set in current_sets):
            continue
        if not any(current_set < baseline_set for current_set in current_sets):
            progress.append(f"rust_feature_cycles: removed cycle containing {', '.join(cycle)}")
    return failures, progress


def should_fail(failures: list[str], baseline_updates: list[str]) -> bool:
    return bool(failures or baseline_updates)


def compare_baseline_ratchet(
    candidate: dict,
    previous: dict,
) -> list[str]:
    """Reject baseline increases relative to the PR target branch."""
    failures: list[str] = []
    if previous.get("schema_version") != candidate.get("schema_version"):
        failures.append(
            "baseline schema changed; migrate the guard and baseline in an explicitly "
            "reviewed architecture change"
        )
        return failures
    previous_rules = previous.get("rules", {})
    for rule, candidate_counts in candidate.get("rules", {}).items():
        previous_counts = previous_rules.get(rule, {})
        for key, count in candidate_counts.items():
            old_count = previous_counts.get(key, 0)
            if count > old_count:
                failures.append(
                    f"baseline ratchet: {rule}: {key}: candidate allows {count}, "
                    f"target branch allows {old_count}"
                )
    previous_cycles = previous.get("rust_feature_cycles", [])
    for cycle in candidate.get("rust_feature_cycles", []):
        if not cycle_allowed(cycle, previous_cycles):
            failures.append(
                "baseline ratchet: cycle is new or wider than target branch: "
                + " -> ".join(cycle)
            )
    return failures


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--root",
        type=Path,
        default=Path(__file__).resolve().parent.parent,
        help="repository root (defaults to the script's parent repository)",
    )
    parser.add_argument(
        "--baseline",
        type=Path,
        default=DEFAULT_BASELINE,
        help="baseline path, relative to repository root by default",
    )
    parser.add_argument(
        "--print-current",
        action="store_true",
        help="print the current findings as baseline JSON without checking",
    )
    parser.add_argument(
        "--base-ref",
        help="also require the checked-in baseline to only tighten relative to this Git ref",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    root = args.root.resolve()
    state = current_state(root)
    if args.print_current:
        print(json.dumps(state, indent=2, ensure_ascii=False))
        return 0

    baseline_path = args.baseline
    if not baseline_path.is_absolute():
        baseline_path = root / baseline_path
    if not baseline_path.exists():
        print(f"architecture guard: baseline not found: {baseline_path}", file=sys.stderr)
        return 2
    try:
        baseline = json.loads(read_text(baseline_path))
    except (OSError, json.JSONDecodeError) as error:
        print(f"architecture guard: invalid baseline: {error}", file=sys.stderr)
        return 2

    failures, progress = compare(state, baseline)
    if args.base_ref:
        try:
            relative_baseline = baseline_path.resolve().relative_to(root)
            previous_baseline = baseline_from_git(root, args.base_ref, relative_baseline)
        except (ValueError, OSError) as error:
            print(f"architecture guard: cannot compare target baseline: {error}", file=sys.stderr)
            return 2
        if previous_baseline is None:
            print(
                f"INFO: {args.base_ref} has no architecture baseline; accepting initial baseline"
            )
        else:
            failures.extend(
                compare_baseline_ratchet(baseline, previous_baseline)
            )
    if should_fail(failures, progress):
        print("architecture guard failed:", file=sys.stderr)
        if failures:
            print("new architecture debt is not allowed:", file=sys.stderr)
            for message in failures:
                print(f"  - {message}", file=sys.stderr)
        if progress:
            print(
                "architecture debt decreased; tighten the matching baseline in this change:",
                file=sys.stderr,
            )
            for message in progress:
                print(f"  - {message}", file=sys.stderr)
        print(
            "Fix new violations. Lower/remove stale baseline entries when debt decreases. "
            "Increase a baseline only in a dedicated, reviewed architecture decision.",
            file=sys.stderr,
        )
        return 1
    print("architecture guard passed: no architecture debt increased")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
