# Retained FullPack native inspection

Independent composition of existing retained download custody, signed Package
binding, FullPack/member copying and Windows embedded-signature observations.
This tool never executes a native file. Outputs are ordinary cache observations,
not protected staging or installation permissions.

The original `inspect` command and library API keep their original report.
`inspect-execution-helper`, `inspect-launcher` and `inspect-helper-artifact`
complete the same pipeline for exact Package role declarations, with a separate
selection wrapper. Execution helper ID comes only from signed Package content.

See [usage and boundaries](../../../../../../../docs/client-upgrade-retained-native-inspection.md).

```text
cargo fmt -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets -- --test-threads=1
```

Windows uses actual signed system PE bytes, independent temporary Ed25519
Package/credential signatures, actual ZIP32 and the existing download CLI source
for original job creation. No fixture image is executed and no certificate store
or service is changed. Other platforms explicitly return unsupported. Strict
native positives require preexisting complete cache-only chains/timestamps and
are separate opt-in tests; they never relax policy or install a test root.
Role tests additionally cover distinct first/execution helpers, arbitrary declared
helper ID, launcher, bad declarations, actual partial-copy cancellation and kill/
wait. Original pre-role download binary compatibility is a separate explicit
opt-in requiring its independently retained exact binary hash.
