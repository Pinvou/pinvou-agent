# ZIP reader compatibility

The application and spreadsheet reader use zip9.0.3 or a compatible non-yanked
zip9 release. A temporary, MIT-licensed calamine 0.36.1 compatibility patch is
kept in `third_party/rust/calamine`, with official provenance and removal
conditions recorded beside the unchanged upstream license.

ZIP filenames follow the upstream zip9 decoding rules. Valid UTF8 filename
bytes are preferred even when an older archive omitted the UTF8 flag;
otherwise bytes decode as CP437. Invalid Unicode path extra fields are ignored
and the header name is used. Historical archives with inconsistent flags or
invalid Unicode extras can therefore produce different names than zip8, or
be accepted where zip8 rejected them. No automatic legacy decoder or silent
entry skipping is added. Re-export ambiguous old archives using correctly
flagged UTF8 names when stable cross-tool names are needed.

Tests cover stored/deflated XLSX and ODS public reads, normal Unicode entry
names, the accepted decoding boundaries, and existing CP437 name-lookup limits.
They do not claim the omitted upstream integration dataset or every malformed
archive is covered. The original 42 inline tests remain, with one cache/Result
regression and a named `zip9_compatibility` target. CI runs this target through
the knowledge Rust job, and changes anywhere in the vendored package route to
all Rust consumers and their dependency gates. No cargo-deny exception is used.
