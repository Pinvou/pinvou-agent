# ZIP dependency security

The application directly uses ZIP 9.0.4, and both the application and knowledge
crate use calamine from the fixed revision
`fa0f5867a092f240b6f77d947ef25f2e3069dfbc` at
https://github.com/zhuowp/calamine. This is a maintainer-controlled fork, not a
published official calamine release. Its official upstream is
https://github.com/tafia/calamine.

The adapter starts from official calamine v0.36.1, commit
`0a24c2a9f1e38c0932c1299e633270dc730db505`. It retains the original MIT license,
quick-xml 0.41, and the original release sources outside the ZIP adapter. It
updates ZIP to 9.0.4, propagates the fallible filename-cache result through the
XLSX/XLSB constructors, and propagates filename errors in the three picture
readers. It does not discard errors with `flatten()`. The original upstream
suite and fixtures are unchanged; nine real archive/Reader tests were added.

ZIP 9 applies its upstream UTF-8/CP437 filename rules. Legacy archives with
incorrect UTF-8 flags can decode differently; invalid Unicode path extras fall
back to header names. Valid Unicode extras remain supported. The existing
CP437 decoded-name/raw-key lookup limitation shared by ZIP 8 and ZIP 9 is not
changed by this adapter. These filename changes are intentional. Calamine
reads workbook data and does not extract archive entries to the filesystem.

The application retains its bounded expected-member extraction and wheel path,
symlink, and size checks. Absolute wheel entry names retain their previous
contained relative-path handling. New production tests cover Unicode ZIP
members, path rejection, symlink metadata, the extraction budget, and absolute
entry containment. Local function-linked validation is separate from full
application and platform-native CI; Windows ZIP metadata tests do not establish
Unix filesystem permission behavior.

The dependency policy again denies yanked releases. Vulnerability advisories,
license checks, bans, unknown registries, and unknown Git sources remain enabled;
only this exact calamine repository is allowed, with full revisions pinned in
both manifests. No RustSec advisory is ignored for this update.

Upstream adaptation is already proposed at
https://github.com/tafia/calamine/pull/741. This release-based adapter deliberately
excludes unrelated changes on upstream master, including quick-xml 0.42. Retire
the fork when an official compatible release is available and its lockfiles and
Office regressions have been verified. Do not merge the release-based branch
back into upstream master, which would revert unrelated work.
