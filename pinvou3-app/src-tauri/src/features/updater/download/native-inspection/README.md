# Retained FullPack native inspection

Independent composition of existing retained download custody, signed Package
binding, FullPack/member copying and Windows embedded-signature observations.
This tool never executes a native file. Outputs are ordinary cache observations,
not protected staging or installation permissions.

See [usage and boundaries](../../../../../../../docs/client-upgrade-retained-native-inspection.md).

```text
cargo fmt -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets -- --test-threads=1
```

Windows uses actual signed system PE bytes, independent temporary Ed25519
Package/credential signatures, actual ZIP32 and the existing download CLI source
for original job creation. No fixture image is executed and no certificate store
or service is changed. Other platforms explicitly return unsupported. One strict
native positive requires preexisting complete cache-only chains/timestamps and
is a separate opt-in test; it never relaxes policy or installs a test root.
