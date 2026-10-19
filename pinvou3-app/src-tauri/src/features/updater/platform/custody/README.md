# Updater-local file custody

Small reusable filesystem adapter shared by retained download jobs and native
signature observations. This is not a system-protected owner or execution seal.

Windows retains actual ancestor handles and uses single-component relative
`NtCreateFile`/`NtSetInformationFile` operations. A held parent that becomes a
junction fails closed; path checks before/after a path operation are not used
as an ABA defense. The implementation is extracted from the reviewed T29
adapter rather than copied into the native verifier. Non-Windows is explicitly
unsupported.

`Directory` supplies bounded-name open/create and no-replace link publication;
caller access arguments select ordinary IO sharing. `identity` requires one
link; `read_identity` permits the original two-name publication profile.
`Identity` retains the existing camelCase JSON representation. Download-specific
errors and journals remain in the T29 feature, with an explicit error adapter.

Tests of actual namespace/custody composition remain mandatory in T29 and the
native evidence leaf. This crate also checks the serialized compatibility
profile. No independent OS implementation or business policy is introduced.
