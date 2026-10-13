# Client upgrade complete envelope Schema

T27 batch15 consumes an original CanonicalEnvelopeShape into privately constructed
SchemaCheckedEnvelope after complete frozen Draft2020-12 role Schema validation.
The20 envelope schemas and common resource are embedded directly with include_str
from their original committed paths, without copied or edited normative artifacts.
A private OnceLock caches either the compiled20-role registry or a fixed failure.
No dynamic schema, URI or caller-supplied envelope substitute is accepted. The
original factory's own bounded raw bytes are reparsed for whole-envelope validation.
SchemaCheckedEnvelope preserves the original shape and provides one immutable
getter, without public constructor, Debug or Serialize.

Use the exact parent jsonschema0.46.10/default-features=false graph (MSRV1.83,
compatible with project1.89), Draft2020-12 and format validation disabled matching
original Ajv. The existing default FancyRegex/ECMA backend supports frozen relative
path negative lookahead. The explicit retriever accepts only the exact embedded
common URN, refusing http/file/unknown resources even if another parent enables
network features. It never interprets payload $ref fields as schema. Errors and
cached compilation failures expose fixed diagnostics without original values,
schema details or panic. Production source has no external resource reads.

Complete Schema success remains independent of signature validity, Root policy/
key consistency, expected context, clock/time semantics, protected acceptance,
high-water persistence or current execution rights. Valid Schema with a zero
signature or reversed issued/expiry values remains a positive Schema example.
Old partial shapes remain explicitly partial; new complete Schema validation is
opt-in through its own typed result, not silently added to previous pure helpers. Parsed temporary values, retained
original bytes and caller buffers have no certified secure-erasure guarantee.

Parent App/CLI lock nodes, versions, checksums and existing edges stay unchanged:
only codec->existing jsonschema is added. The isolated validator closure is pruned
from the existing parent lock, then resolved offline. All114 isolated packages use
exact parent versions/checksums; six previously differing isolated transitive
versions align to parent: cfg-if1.0.5->1.0.4, cpufeatures0.3.1->0.3.0,
hybrid-array0.4.15->0.4.14, libc0.2.190->0.2.189, syn3.0.8->3.0.5 and
unicode-ident1.0.27->1.0.24. This is explicitly not a claim that all old isolated
versions remain unchanged. The initial hand-selected125-package closure failed
--locked because unused/unified dependencies needed pruning; offline resolution
produced the114-package parent subset, with no external upgrade.

Two integration groups plus one resource-refusal unit test cover20 original vectors and full-schema-negative minimal bodies, nested
required/closed/ref/type/scalar/array variants, numeric SAFE and Unicode/regex/
relative-path cases, canonical/encoding preconditions, fixed resource refusal and
Schema-only semantics. The31556-case actual Node oracle reuses original parseEnvelope plus
decodeBase64Url and actual validateSchema, comparing the full composed acceptance
pipeline. The oracle's expected context is derived from each case's own signed
fields solely to isolate Schema validation; it is not a trusted expected-context
source or a production verifier. Production expected-context validation remains
external. AJV alone accepts inputs refused by canonical/encoding preconditions and
is not the claimed equivalence domain. App facades check both profiles, original
bytes, extra-field refusal and zero-signature Schema-only acceptance.

Final verification and independent evidence appear in
[implementation progress](upgrade-implementation-progress.md).
Complete T27 authenticity and persistence remain unfinished. Downloads retain no
total/cumulative duration or retry cap.
