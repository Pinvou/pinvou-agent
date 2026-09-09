# macOS Keychain authorization during development

The password dialog authorizes the application to read a stored credential. It is
not a model-provider login. The password belongs to the macOS login keychain;
application code must not collect or persist that password.

## Why rebuilds can prompt again

The default community build uses ad-hoc signing (`signingIdentity = "-"`). A new
binary has a different code-directory hash. Keychain access checks both code
requirements and partition membership. For code that is not recognized as
Apple-issued development/distribution code, the partition can be based on its
`cdhash`. Consequently, a self-signed certificate with a stable designated
requirement is **not sufficient** to guarantee access across rebuilds.

This was reproduced with a disposable Keychain item: a self-signed build could
create/read its item, but a differently compiled build signed by the same
certificate failed with interaction disabled. Existing model credentials were
not used for that test.

References:
- [Apple code-signing requirements](https://developer.apple.com/library/archive/documentation/Security/Conceptual/CodeSigningGuide/RequirementLang/RequirementLang.html)
- [Apple Security partition selection](https://github.com/apple-oss-distributions/Security/blob/main/securityd/src/clientid.cpp)
- [Apple Keychain authorization](https://support.apple.com/en-ca/guide/keychain-access/kyca1243/mac)

## Configure development signing

1. Install an **Apple Development** or **Developer ID Application** identity,
   including its private key, in the current user's keychain. Xcode's Settings →
   Accounts → Manage Certificates is the usual entry point for development
   certificates. A standalone certificate without its private key is insufficient.
2. Check availability: `security find-identity -v -p codesigning`.
3. Run `npm --prefix pinvou3-app run setup:macos-signing`.
   If several identities are available, select the intended certificate using
   `PINVOU3_MACOS_SIGNING_IDENTITY=<SHA-1 fingerprint>` for this setup command.
4. Restart the development launcher using `./pinvou3-app/run-dev.sh` or
   `npm --prefix pinvou3-app run dev`.
5. For existing credentials, macOS may require one new authorization for the
   Apple-signed application. Select **Always Allow** if you trust this build.
   Different credential items can require separate initial authorizations.

Setup saves only the public certificate fingerprint in
`~/Library/Application Support/fresh-agent/development-signing.json` with mode
0600. It does not generate certificates, import private keys, change trust
settings, or loosen credential access rules.

The normal macOS development launcher injects Cargo target runners for Apple
Silicon and Intel. After compilation, the runner signs and verifies the executable
before using `exec` to launch it with the original arguments. This covers both
fresh builds and no-op restarts, including Rust watch rebuilds. The runner preserves
Tauri's process lifecycle and never starts the application after a signing failure.

The setup command rejects missing and self-signed identities. Once configured, a
missing/expired identity or locked signing key stops launch with an actionable
error. Without setup, the normal development command retains ad-hoc behavior and
prints a setup hint. `PINVOU3_MACOS_DEV_SIGNING=0` explicitly disables development
signing for troubleshooting; repeated authorization after rebuilding may return.

## Verification and limits

Run the deterministic launcher tests:

```bash
node --test pinvou3-app/tests/macos_dev_signing.test.js
```

After installing a supported identity, run the native integration test:

```bash
PINVOU3_TEST_MACOS_SIGNING=1 node --test pinvou3-app/tests/macos_dev_signing.test.js
```

The opt-in test creates a uniquely identified disposable credential, disables
Keychain interaction, rebuilds and re-signs a test executable, and verifies reads
from new processes before deleting only its own test item. It does not read model
keys. Without a supported Apple identity this native test cannot validate the fix.

The application retains its process-wide credential cache and per-key locks in
`credential_store.rs`. Successful reads are cached; denied or failed reads can be
retried. Locking the login keychain, changing access rules or changing signing team
can still require authorization. An unchanged application is not guaranteed to
prompt on every restart merely because it has an ad-hoc signature.

This development runner does not alter release signing, Windows/Linux builds, the
bundle identifier, stored credential names or data paths. Distributed macOS builds
still need a suitable Developer ID signing and notarization workflow. Do not solve
authorization prompts by permitting every application to read model credentials
or by copying those credentials into plaintext configuration files.
