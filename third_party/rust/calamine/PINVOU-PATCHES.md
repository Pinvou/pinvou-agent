# calamine 0.36.1: temporary zip9 compatibility patch

Source: the official crates.io `calamine-0.36.1.crate`; archive checksum and original published-file hashes are recorded in `UPSTREAM_SOURCE.json`. `LICENSE-MIT.md`, authors, and upstream attribution remain unchanged. `Cargo.toml.orig` is the original upstream manifest, including its original zip8 constraint; Cargo consumes the patched normalized `Cargo.toml` instead.

The normalized manifest selects zip9.0.3 with the same `default-features = false` and `deflate` feature. `build_zip_path_cache` now returns `ZipResult`, propagates filename errors without skipping entries, and the existing XLSX/XLSB constructors preserve their original public error types through their existing ZipError conversion. One original unit-test call unwraps its known valid archive. Picture-feature readers in ODS/XLSX/XLSB likewise propagate decoded-name errors with `?`; no entry is silently dropped. All 42 published inline lib tests are retained; a cache/Result regression and the explicit `zip9_compatibility` target are added.

The compatibility tests use minimal independently constructed stored/deflated XLSX/ODS fixtures and fixed ZIP bytes. They exercise public spreadsheet readers, normal Unicode ZIP entry names, unflagged UTF8 names, malformed UTF8 with the UTF8 flag, and invalid Unicode extra-field CRC/payload. zip9 upstream rules are intentional: prefer valid UTF8 filename bytes, otherwise CP437; ignore invalid Unicode path extras and fall back to the header name. These can differ from zip8 historical names. CP437 decoded-name lookup failure is an existing limitation in both versions, not a new compatibility guarantee. No zip8 production dependency or automatic fallback decoder is included.

The published crate excludes `tests/**/*`; its upstream integration fixtures are not included here. The upstream benchmark and examples are preserved byte for byte, including references to upstream fixture paths; they are not a claim that the omitted upstream dataset is bundled. Use `cargo test --lib --test zip9_compatibility --all-features --locked` for the published inline tests and the local bounded regressions. `cargo check --all-targets` checks target compilation without executing fixture-dependent benchmarks. This is not full upstream or App/Tauri/native workspace acceptance.

Remove this vendored override when a published calamine release accepts a non-yanked zip release with the required reader APIs, the same bounded regressions pass, and all three product workspace lockfiles and applicable gates validate the replacement. Do not relax cargo-deny or reintroduce yanked zip to remove the override.

`PINVOU.patch` records the five upstream manifest/source deltas as a zero-context
patch (verify `UPSTREAM_SOURCE.json` first; apply with `git apply --unidiff-zero`).
Additional local tests/fixtures and provenance files are separate. The original
CRLF example, one whitespace-only upstream workflow line, and the example README
blank line at EOF are preserved byte for byte; `.gitattributes` names only those
three files for whitespace handling.
