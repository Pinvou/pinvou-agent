# Client upgrade exact declared role policy

T27 batch12 retains a private immutable typed product/policy cache inside the
supplied RootPolicyShape. The existing Root validator constructs each policy in
the same validated pass, preserves keyIds order and the complete original Root
canonical bytes, and checks threshold conversion to u16 only after positive safe
integer and threshold<=members<=4096 validation. No large Root JSON clone or
alternate public construction path is introduced.

RoleContextShape parses the original bounded supplied signed-body Document,
requiring string role/product, explicit null-or-string component and object scope.
Only missing/null scope.channel or scope.targetKey become null; all strings
remain exact, with no trim, case conversion, enums, wildcard or default. Other
scope/body fields are ignored by this partial context function but retain whole
raw resource/duplicate checks. It does not unwrap an envelope. Object scope is the
existing V1 claims precondition; equivalence to original policyFor for primitive
scope outside that domain is not claimed. Full claims Schema remains external.

Root lookup first requires exact product, then exact role/component/channel/
targetKey tuple. Root shape validation already proves tuple uniqueness. No
generic fallback, scope union or synthesized policy exists. Borrowed private
DeclaredRolePolicy lives no longer than its Root shape; threshold, original
member IDs and bounded canonical policy getters expose declared facts only.
Types have no Debug/Serialize or public constructor. Neither the Root shape nor
a matching declared policy proves trusted keys, signature threshold satisfaction,
role authorization, Root authenticity/currentness or execution rights.

Four groups cover20 roles, same-role scopes/components, independent product and
tuple mismatches, explicit/missing component and nullable scope fields, member
order/threshold,4096-policy lookup and4096-member/threshold getters, ignored-extra
resources/escaped duplicates and exact/+1 context wire limits. The Node oracle
reads the frozen signatures.mjs private policyFor function body unchanged and
executes it with vm.compileFunction plus the original requireCondition helper.
Extraction must succeed; no copied matching algorithm or fallback exists.
Original validateTrustedRoot and the explicitly stated context subset precondition
precede the call. No reference file is modified. All639 original Root validation
cases remain byte/acceptance regressions. An actual App facade checks exact
selection and refuses a changed product or undeclared channel.

No new dependency, lock, Schema, CI, product version or gitlink is needed. Final
evidence appears in [implementation progress](upgrade-implementation-progress.md).
Complete T27 signature/trust/lineage/high-watermark integration remains unfinished.
Downloads retain no total/cumulative duration or retry cap.
