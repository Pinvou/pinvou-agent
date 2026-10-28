import importlib.util
import json
from pathlib import Path
import re
import shlex
import subprocess
import sys
import tempfile
import tomllib
import unittest
from unittest import mock


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts/guard-webpki-roots-exception.py"
SPEC = importlib.util.spec_from_file_location("webpki_guard", SCRIPT)
GUARD = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GUARD)
WORKFLOW = ROOT / ".github/workflows/pr-check.yml"
EXPECTED_LOCKFILES = (
    "pinvou-knowledge/Cargo.lock",
    "pinvou3-app/src-tauri/Cargo.lock",
    "pinvou-cli/Cargo.lock",
)


class WebpkiExceptionTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        for relative in EXPECTED_LOCKFILES:
            target = self.root / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes((ROOT / relative).read_bytes())

    def mutate_roots(self, relative, version, transform):
        path = self.root / relative
        text = path.read_text(encoding="utf-8")
        pattern = rf'\[\[package\]\]\nname = "webpki-roots"\nversion = "{re.escape(version)}"\n.*?(?=\n\[\[package\]\]|\Z)'
        text, count = re.subn(pattern, lambda match: transform(match[0]), text, flags=re.S)
        self.assertEqual(count, 1, "mutation must hit an actual package exactly once")
        path.write_text(text, encoding="utf-8")

    def test_three_real_locks_pass_without_modification(self):
        self.assertEqual(GUARD.LOCKFILES, EXPECTED_LOCKFILES)
        before = {path: (ROOT / path).read_bytes() for path in EXPECTED_LOCKFILES}
        GUARD.validate_repository(ROOT)
        result = subprocess.run([sys.executable, str(SCRIPT)], cwd=self.root, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(before, {path: (ROOT / path).read_bytes() for path in EXPECTED_LOCKFILES})

    def assert_every_consumer_required(self):
        # Expected consumers are independent of the production selector, so
        # accidentally deleting a selector cannot shrink this test with it.
        for relative in EXPECTED_LOCKFILES:
            path = self.root / relative
            original = path.read_bytes()
            path.unlink()
            try:
                with self.assertRaises(GUARD.GuardError):
                    GUARD.validate_repository(self.root)
            finally:
                path.write_bytes(original)

    def test_every_consumer_is_required_and_not_selectable(self):
        self.assert_every_consumer_required()
        result = subprocess.run([sys.executable, str(SCRIPT), EXPECTED_LOCKFILES[0]], capture_output=True, text=True)
        self.assertEqual(result.returncode, 1)

    def test_removing_each_production_consumer_breaks_the_missing_lock_contract(self):
        for omitted in EXPECTED_LOCKFILES:
            with self.subTest(omitted=omitted), mock.patch.object(
                GUARD, "LOCKFILES", tuple(path for path in EXPECTED_LOCKFILES if path != omitted)
            ):
                with self.assertRaises(AssertionError):
                    self.assert_every_consumer_required()

    def test_downgrades_and_unreviewed_versions_fail_in_each_real_lock(self):
        for relative in EXPECTED_LOCKFILES:
            original = (self.root / relative).read_bytes()
            for old, new in [("1.0.11", "1.0.10"), ("1.0.11", "1.0.12"), ("0.26.11", "0.26.10"), ("0.26.11", "0.26.12")]:
                with self.subTest(lock=relative, version=new):
                    self.mutate_roots(relative, old, lambda block: block.replace(f'version = "{old}"', f'version = "{new}"'))
                    with self.assertRaises(GUARD.GuardError):
                        GUARD.validate_repository(self.root)
                    (self.root / relative).write_bytes(original)

    def test_edges_sources_and_checksums_fail_closed(self):
        changes = [
            ("0.26.11", lambda block: block.replace('"webpki-roots 1.0.11"', '"webpki-roots 1.0.9"')),
            ("0.26.11", lambda block: block.replace('"webpki-roots 1.0.11"', '"webpki-roots"')),
            ("0.26.11", lambda block: block.replace('"webpki-roots 1.0.11",', '"webpki-roots 1.0.11",\n "rustls-pki-types",')),
            ("1.0.11", lambda block: block.replace('"rustls-pki-types",', '')),
        ]
        for version in GUARD.REVIEWED:
            changes.extend([
                (version, lambda block: block.replace(GUARD.REGISTRY, "git+https://example.invalid/roots")),
                (version, lambda block: re.sub(r'^source = .*\n', '', block, flags=re.M)),
                (version, lambda block: re.sub(r'checksum = "[a-f0-9]+"', 'checksum = "' + '0' * 64 + '"', block)),
                (version, lambda block: re.sub(r'^checksum = .*\n', '', block, flags=re.M)),
            ])
        for relative in EXPECTED_LOCKFILES:
            original = (self.root / relative).read_bytes()
            for index, (version, transform) in enumerate(changes):
                with self.subTest(lock=relative, mutation=index):
                    self.mutate_roots(relative, version, transform)
                    with self.assertRaises(GUARD.GuardError):
                        GUARD.validate_repository(self.root)
                    (self.root / relative).write_bytes(original)

    def test_missing_extra_duplicate_and_ambiguous_packages_fail(self):
        relative = EXPECTED_LOCKFILES[0]
        path = self.root / relative
        original = path.read_bytes()
        for transform in [lambda block: '', lambda block: block + '\n' + block,
                          lambda block: block + '\n' + block.replace(GUARD.REGISTRY, 'git+https://example.invalid/roots'),
                          lambda block: block + '\n' + block.replace('version = "0.26.11"', 'version = "0.25.0"')]:
            self.mutate_roots(relative, "0.26.11", transform)
            with self.assertRaises(GUARD.GuardError):
                GUARD.validate_repository(self.root)
            path.write_bytes(original)

    def test_unreadable_or_malformed_lock_is_not_empty_success(self):
        path = self.root / EXPECTED_LOCKFILES[1]
        for text in ['', 'version = 3\n', 'version = 4\npackage = "bad"', '[broken']:
            path.write_text(text, encoding="utf-8")
            with self.assertRaises(GUARD.GuardError):
                GUARD.validate_repository(self.root)

    def test_lock_format_version_requires_an_integer(self):
        with (ROOT / EXPECTED_LOCKFILES[0]).open('rb') as stream:
            value = tomllib.load(stream)
        for version in [4.0, '4', True]:
            with self.subTest(version=version), self.assertRaises(GUARD.GuardError):
                GUARD.validate_lock({**value, 'version': version}, 'fixture')

    def test_invalid_utf8_and_deep_nesting_are_clean_guard_failures(self):
        path = self.root / EXPECTED_LOCKFILES[0]
        for content in [b'\xff', b'x = ' + b'[' * 2000 + b'0' + b']' * 2000]:
            path.write_bytes(content)
            with self.assertRaises(GUARD.GuardError):
                GUARD.validate_repository(self.root)

    def assert_cli_rejects_bad_locks(self, entry):
        original = {path: (self.root / path).read_bytes() for path in EXPECTED_LOCKFILES}
        cases = ['missing_cli', 'old_roots', 'old_parent', 'wrong_checksum']
        for case in cases:
            try:
                if case == 'missing_cli':
                    (self.root / EXPECTED_LOCKFILES[2]).unlink()
                elif case == 'old_roots':
                    self.mutate_roots(EXPECTED_LOCKFILES[0], '1.0.11',
                                      lambda block: block.replace('version = "1.0.11"', 'version = "1.0.9"'))
                elif case == 'old_parent':
                    self.mutate_roots(EXPECTED_LOCKFILES[1], '0.26.11',
                                      lambda block: block.replace('webpki-roots 1.0.11', 'webpki-roots 1.0.9'))
                else:
                    self.mutate_roots(EXPECTED_LOCKFILES[2], '1.0.11',
                                      lambda block: block.replace(GUARD.REVIEWED['1.0.11']['checksum'], '0' * 64))
                snapshot = {path: (self.root / path).read_bytes() if (self.root / path).exists() else None
                            for path in EXPECTED_LOCKFILES}
                # cwd deliberately differs from the copied entry's repository.
                result = subprocess.run([sys.executable, str(entry)], cwd=ROOT, capture_output=True, text=True)
                self.assertNotEqual(result.returncode, 0, case)
                self.assertEqual(snapshot, {path: (self.root / path).read_bytes() if (self.root / path).exists() else None
                                            for path in EXPECTED_LOCKFILES})
            finally:
                for path, content in original.items():
                    (self.root / path).write_bytes(content)

    def test_real_cli_entry_rejects_bad_locks_without_writes(self):
        entry = self.root / 'scripts/guard-webpki-roots-exception.py'
        entry.parent.mkdir()
        entry.write_bytes(SCRIPT.read_bytes())
        self.assert_cli_rejects_bad_locks(entry)

    def test_removing_main_validation_is_caught_by_cli_negative_control(self):
        entry = self.root / 'scripts/guard-webpki-roots-exception.py'
        entry.parent.mkdir()
        source = SCRIPT.read_text(encoding='utf-8')
        target = 'validate_repository(Path(__file__).resolve().parents[1])'
        self.assertEqual(source.count(target), 1)
        entry.write_text(source.replace(target, 'pass'), encoding='utf-8')
        with self.assertRaises(AssertionError):
            self.assert_cli_rejects_bad_locks(entry)


def job_block(workflow, name):
    match = re.search(rf'^  {re.escape(name)}:\n(.*?)(?=^  [\w-]+:|\Z)', workflow, re.M | re.S)
    if not match:
        raise AssertionError(f"missing job {name}")
    return match[1]


def cargo_deny_commands(workflow):
    """Inspect actual run scalars, including block and quoted single-line runs.

    This deliberately supports the workflow's explicit scalar style, not YAML
    aliases or arbitrary objects. Unrecognized run forms require a test update.
    """
    lines = workflow.splitlines()
    commands = []
    scalar_lines = set()
    reviewed_actions = {'actions/checkout', 'actions/setup-node', 'actions/setup-python',
                        'actions/cache/restore', 'actions/cache/save', 'dorny/paths-filter',
                        'dtolnay/rust-toolchain', 'Swatinem/rust-cache', 'taiki-e/install-action'}
    for index, line in enumerate(lines):
        if index in scalar_lines or line.lstrip().startswith('#'):
            continue
        # This is an explicit workflow dialect, not a YAML parser. Reject
        # alternate mapping keys before they can disappear from the inventory.
        key = re.match(r'''^\s*(?:-\s*)?(?:\{\s*)?['"]?(run|uses)['"]?\s*:''', line)
        if re.match(r'^\s*-\s*\{', line):
            raise AssertionError('unsupported flow step mapping; extend the entry contract')
        if key and not re.match(r'^\s+(?:-\s+)?(?:run|uses):', line):
            raise AssertionError('unsupported run/uses mapping key; extend the entry contract')
        action = re.match(r'^\s+(?:-\s+)?uses:\s*(.*?)\s*$', line)
        if action and action[1].split('@')[0] not in reviewed_actions:
            raise AssertionError('unreviewed action or reusable workflow; inspect cargo-deny coverage')
        match = re.match(r'^(\s+)(?:-\s+)?run:\s*(.*?)\s*$', line)
        if not match:
            continue
        scalar = match[2]
        # In "- run:", the mapping key starts after the list marker. A
        # sibling step field at that column is not scalar continuation text.
        run_indent = len(match[1]) + (2 if line.lstrip().startswith('- ') else 0)
        # defaults.run is a mapping (shell/cwd), not an executable step.
        if not scalar and index > 0 and lines[index - 1].strip() == 'defaults:':
            continue
        if scalar in ('|', '|-', '|+', '>', '>-', '>+'):
            body = []
            for body_index, following in enumerate(lines[index + 1:], index + 1):
                if following.strip() and len(following) - len(following.lstrip()) <= run_indent:
                    break
                body.append(following)
                scalar_lines.add(body_index)
            scalar = '\n'.join(body)
        else:
            # YAML folds multiline plain scalars into one command. Do not
            # inspect only the first word and silently miss a later "deny".
            for following in lines[index + 1:]:
                if not following.strip() or following.lstrip().startswith('#'):
                    continue
                indent = len(following) - len(following.lstrip())
                sibling = indent == run_indent and re.match(r'[\w-]+:', following.lstrip())
                if indent > len(match[1]) and not sibling:
                    raise AssertionError('unsupported multiline run scalar; use an explicit block')
                break
            if scalar.startswith('"'):
                scalar = json.loads(scalar)
            elif scalar.startswith("'"):
                if not scalar.endswith("'"):
                    raise AssertionError('unterminated run scalar')
                scalar = scalar[1:-1].replace("''", "'")
            elif not scalar or scalar.startswith(('*', '&', '{', '[', '|', '>', '!', '#')):
                raise AssertionError('unrecognized run scalar; inspect its cargo-deny coverage')
        # Bash removes quotes/escapes from command words before execution.
        # Searching source text misses e.g. 'cargo' 'deny' or ca"rgo" deny.
        # This is lexical change control, not an arbitrary-shell evaluator.
        try:
            lexer = shlex.shlex(scalar.replace('\\\n', ''), posix=True, punctuation_chars=True)
            lexer.whitespace_split = True
            words = list(lexer)
        except ValueError as error:
            raise AssertionError('unrecognized shell words; inspect cargo-deny coverage') from error
        commands.extend('cargo deny' for first, second in zip(words, words[1:])
                        if first == 'cargo' and second == 'deny')
        commands.extend('cargo-deny' for word in words if word == 'cargo-deny')
    return commands


class WebpkiCiContractTests(unittest.TestCase):
    def verify_workflow(self, workflow):
        workflow = '\n'.join(line for line in workflow.splitlines()
                             if not line.lstrip().startswith('#')) + '\n'
        self.assertEqual(len(cargo_deny_commands(workflow)), 2,
                         'a new cargo-deny entry must join this guarded-entry contract')
        cases = [
            ("knowledge-rust", "cargo deny check", "needs.changes.outputs.knowledge_dependencies == 'true'",
             "python3 scripts/guard-webpki-roots-exception.py",
             "cargo deny --locked --manifest-path pinvou-knowledge/Cargo.toml --config pinvou3-app/src-tauri/deny.toml check advisories licenses bans sources"),
            ("rust-lint", "cargo deny check (hard gate)", "env.RUN_HEAVY_RUST_CHECKS == 'true' && needs.changes.outputs.rust_dependencies == 'true'",
             "python3 ../../scripts/guard-webpki-roots-exception.py", "cargo deny --locked check advisories licenses bans sources"),
        ]
        for job, step_name, condition, guard, deny in cases:
            block = job_block(workflow, job)
            steps = block.split('\n      - ')
            scan = next(step for step in steps if step.startswith(f'name: {step_name}\n'))
            setup = next(step for step in steps if step.startswith('name: Python for dependency exception guard\n'))
            self.assertLess(steps.index(setup), steps.index(scan))
            for step in [scan, setup]:
                self.assertIn('        if: ${{ ' + condition + ' }}\n', step)
                self.assertNotIn('continue-on-error', step)
            self.assertIn('uses: actions/setup-python@v7', setup)
            self.assertIn("python-version: '3.12'", setup)
            run = re.search(r'        run: \|\n((?:          [^\n]*\n|\n)*)', scan)
            self.assertIsNotNone(run)
            commands = [line.strip() for line in run[1].splitlines() if line.strip()]
            self.assertEqual(commands, [guard, deny], "guard must succeed before the locked scan in the same step")
            self.assertEqual(re.findall(r'^        shell: (.*)$', scan, re.M), ['bash'],
                             'pin the built-in fail-fast shell above job/workflow defaults')
            self.assertEqual(re.findall(r'^        working-directory: (.*)$', scan, re.M),
                             ['.' if job == 'knowledge-rust' else 'pinvou3-app/src-tauri'],
                             'pin the audited lock and configuration directory')
        changes = job_block(workflow, 'changes')
        for group in ['rust_dependencies', 'knowledge_dependencies']:
            block = re.search(rf'^            {group}:\n(.*?)(?=^            \w+:|\Z)', changes, re.M | re.S)[1]
            for path in ['scripts/guard-webpki-roots-exception.py', 'scripts/tests/test_webpki_roots_exception.py']:
                self.assertIn(f"- '{path}'", block)
                self.assertNotIn(f"- '!{path}'", block)
        fast = job_block(workflow, 'fast-gate')
        self.assertIn("python3 -m unittest discover -s scripts/tests -p 'test_*.py'", fast)

    def test_both_shared_policy_consumers_have_mandatory_locked_scan(self):
        self.verify_workflow(WORKFLOW.read_text(encoding='utf-8'))

    def test_unrecognized_run_headers_cannot_hide_an_unguarded_scan(self):
        workflow = WORKFLOW.read_text(encoding='utf-8')
        for header in ['| # audit', '|2', '|2-', '>|', '>- # audit', '!!str |', '&audit |']:
            with self.subTest(header=header):
                changed = workflow + ('\n  unguarded-audit:\n    steps:\n'
                                      f'      - run: {header}\n'
                                      '          cargo deny check advisories\n')
                with self.assertRaises(AssertionError):
                    self.verify_workflow(changed)

    def test_alternate_mapping_keys_and_actions_require_explicit_review(self):
        workflow = WORKFLOW.read_text(encoding='utf-8')
        for body in ['- {run: cargo deny check advisories, shell: bash}',
                     "- 'run': cargo deny check advisories", '- "run": cargo deny check advisories',
                     '- run : cargo deny check advisories',
                     '- uses: EmbarkStudios/cargo-deny-action@v2',
                     '- uses: ./.github/actions/audit', '- uses: owner/repo/.github/workflows/audit.yml@main',
                     '- {uses: audit/action@v1}', "- 'uses': audit/action@v1"]:
            with self.subTest(body=body), self.assertRaises(AssertionError):
                self.verify_workflow(workflow + '\n  extra-audit:\n    steps:\n      ' + body + '\n')

    def test_scan_working_directories_cannot_drift_or_inherit_defaults(self):
        workflow = WORKFLOW.read_text(encoding='utf-8')
        for job, step_name in [('knowledge-rust', 'cargo deny check'),
                               ('rust-lint', 'cargo deny check (hard gate)')]:
            scan = next(step for step in job_block(workflow, job).split('\n      - ')
                        if step.startswith(f'name: {step_name}\n'))
            for directory in ['', '        working-directory: pinvou-knowledge\n']:
                changed = re.sub(r'^        working-directory: .*\n', directory, scan, flags=re.M)
                with self.subTest(job=job, directory=directory), self.assertRaises(AssertionError):
                    self.verify_workflow(workflow.replace(scan, changed))

    def test_multiline_plain_runs_cannot_hide_an_unguarded_scan(self):
        workflow = WORKFLOW.read_text(encoding='utf-8')
        for body in [
            '      - run: cargo\n          deny check advisories\n',
            '      - run: cargo\n        deny check advisories\n',
            '      - name: extra scan\n        run: cargo\n          deny check advisories\n',
            '      - run: cargo\n\n          deny check advisories\n',
            '      - run: cargo\n          # continuation follows\n          deny check advisories\n',
        ]:
            with self.subTest(body=body):
                changed = workflow + '\n  unguarded-audit:\n    steps:\n' + body
                with self.assertRaisesRegex(AssertionError, 'multiline run scalar'):
                    self.verify_workflow(changed)

    def test_shell_quoting_cannot_hide_an_unguarded_scan(self):
        workflow = WORKFLOW.read_text(encoding='utf-8')
        for command in ["'cargo' 'deny' check advisories", '"cargo" "deny" check advisories',
                        'ca"rgo" d"eny" check advisories', r'ca\rgo de\ny check advisories',
                        'cargo\\\n deny check advisories',
                        "true;'cargo' 'deny' check advisories",
                        'true&&cargo deny check advisories', 'cargo-deny check advisories']:
            with self.subTest(command=command):
                # json.dumps produces a real double-quoted YAML scalar, not
                # a shell-quoted YAML approximation.
                changed = workflow + ('\n  unguarded-audit:\n    steps:\n'
                                      f'      - run: {json.dumps(command)}\n')
                self.assertEqual(len(cargo_deny_commands(changed)), 3)
                with self.assertRaisesRegex(AssertionError, 'new cargo-deny entry'):
                    self.verify_workflow(changed)

    def test_guarded_scans_cannot_inherit_a_non_fail_fast_shell(self):
        workflow = WORKFLOW.read_text(encoding='utf-8')
        for job, step_name in [('knowledge-rust', 'cargo deny check'),
                               ('rust-lint', 'cargo deny check (hard gate)')]:
            for defaults in ['    defaults:\n      run:\n        shell: bash {0}\n',
                             '    defaults: {run: {shell: "bash {0}"}}\n']:
                with self.subTest(job=job, defaults=defaults):
                    changed = workflow.replace(f'  {job}:\n', f'  {job}:\n{defaults}')
                    self.verify_workflow(changed)
                    # An explicit safe step shell must override either form of
                    # job defaults. Removing it must never silently inherit them.
                    block = job_block(changed, job)
                    scan = next(step for step in block.split('\n      - ')
                                if step.startswith(f'name: {step_name}\n'))
                    unsafe = changed.replace(scan, scan.replace('        shell: bash\n', ''))
                    with self.assertRaises(AssertionError):
                        self.verify_workflow(unsafe)

    def test_workflow_defaults_and_step_shell_mutations_preserve_fail_fast(self):
        workflow = WORKFLOW.read_text(encoding='utf-8')
        changed = 'defaults:\n  run:\n    shell: bash {0}\n' + workflow
        self.verify_workflow(changed)
        for replacement in ['', '        shell: bash {0}\n', '        shell: sh\n',
                            '        shell: python\n']:
            for job, step_name in [('knowledge-rust', 'cargo deny check'),
                                   ('rust-lint', 'cargo deny check (hard gate)')]:
                with self.subTest(job=job, replacement=replacement):
                    scan = next(step for step in job_block(changed, job).split('\n      - ')
                                if step.startswith(f'name: {step_name}\n'))
                    unsafe = changed.replace(scan, scan.replace('        shell: bash\n', replacement))
                    with self.assertRaises(AssertionError):
                        self.verify_workflow(unsafe)

    def test_run_list_marker_does_not_consume_sibling_step_fields(self):
        workflow = WORKFLOW.read_text(encoding='utf-8')
        for scalar in ['echo fixture', '|\n          echo fixture']:
            changed = workflow + ('\n  unrelated-run:\n    steps:\n'
                                  f'      - run: {scalar}\n'
                                  '        env:\n          FIXTURE: value\n')
            self.verify_workflow(changed)

    def test_workflow_mutations_cannot_silently_bypass_guard(self):
        workflow = WORKFLOW.read_text(encoding='utf-8')
        for prefix in ['', '../../']:
            guard = f'          python3 {prefix}scripts/guard-webpki-roots-exception.py\n'
            for replacement in ['', guard.rstrip() + ' || true\n', guard.replace('python3', '# python3')]:
                with self.subTest(prefix=prefix, replacement=replacement):
                    changed = workflow.replace(guard, replacement)
                    self.assertNotEqual(changed, workflow)
                    with self.assertRaises(AssertionError):
                        self.verify_workflow(changed)
        for changed in [workflow.replace('cargo deny --locked', 'cargo deny'),
                        workflow.replace("python-version: '3.12'", "python-version: '3.10'"),
                        workflow.replace("              - 'scripts/guard-webpki-roots-exception.py'\n", ''),
                        re.sub(r'(          python3 [^\n]+\n)(          cargo deny [^\n]+\n)', r'\2\1', workflow),
                        workflow + '\n  unguarded-audit:\n    runs-on: ubuntu-22.04\n    steps:\n      - name: extra scan\n        run: |\n          cargo deny check advisories\n',
                        workflow.replace('          cargo deny --locked check advisories licenses bans sources\n',
                                         '          cargo deny --locked check advisories licenses bans sources\n      - run: cargo deny check advisories\n'),
                        workflow + '\n  unguarded-audit:\n    steps:\n      - run: \'cargo deny check advisories\'\n',
                        workflow + '\n  unguarded-audit:\n    steps:\n      - run: "cargo deny check advisories"\n',
                        workflow.replace('      - name: cargo deny check\n', '      - name: cargo deny check\n        continue-on-error: true\n'),
                        workflow.replace("        if: ${{ needs.changes.outputs.knowledge_dependencies == 'true' }}\n        shell: bash\n        working-directory: .\n        run: |", "        if: ${{ false }}\n        shell: bash\n        working-directory: .\n        run: |"),
                        workflow.replace("        if: ${{ needs.changes.outputs.knowledge_dependencies == 'true' }}\n        shell: bash\n        working-directory: .\n        run: |", "        # old if: ${{ needs.changes.outputs.knowledge_dependencies == 'true' }}\n        if: ${{ false }}\n        shell: bash\n        working-directory: .\n        run: |"),
                        workflow.replace("              - 'scripts/guard-webpki-roots-exception.py'", "              # - 'scripts/guard-webpki-roots-exception.py'")]:
            self.assertTrue(changed != workflow, 'negative control must mutate the workflow')
            with self.assertRaises(AssertionError):
                self.verify_workflow(changed)

    def test_policy_scans_roots_advisory_without_an_ignore_and_keeps_bans(self):
        with (ROOT / 'pinvou3-app/src-tauri/deny.toml').open('rb') as stream:
            policy = tomllib.load(stream)
        ignores = [item if isinstance(item, str) else item['id'] for item in policy['advisories']['ignore']]
        self.assertNotIn('RUSTSEC-2026-0381', ignores)
        self.assertNotIn('RUSTSEC-2026-0383', ignores)
        self.assertIn({'name': 'webpki-roots', 'version': '>=1.0.0, <1.0.11'}, policy['bans']['deny'])
        self.assertIn({'name': 'webpki-roots', 'version': '>=0.26.0, <0.26.11'}, policy['bans']['deny'])


if __name__ == '__main__':
    unittest.main()
