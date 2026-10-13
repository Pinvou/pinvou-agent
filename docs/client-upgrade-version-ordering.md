# Client upgrade canonical version ordering

T27 batch9 validates each operand as an original bounded JSON string containing
exactly three ASCII decimal components. Each is zero or a nonzero initial digit
followed by digits. The existing Source predicate is moved unchanged into the
private shared shape helper; both Source and the new version API use it.
No trim, normalization, prefix, suffix, sign, exponent or non-ASCII digits are
accepted. JSON escapes decode before validation and preserve semantic equality.

CanonicalAppVersion privately stores the decoded string, without raw constructor
or Debug/Serialize. compare_app_versions accepts only two such validated operands
and compares major/minor/patch in order, stopping at the first differing component.
With no leading zeros, component length followed by ASCII lexicographic order is
exactly the frozen Node BigInt ordering. No integer conversion, magnitude bound
or component-length cap is introduced. Work and stored input remain bounded by
the original64KiB decoded string ceiling and complete JSON resource limits.
Metadata and Credential retain their distinct wire limits; a maximal decoded
string plus JSON quotes exceeds the Credential64KiB input ceiling.

Four groups compare actual original Node parseJson/compareVersions results for
both operands, including every component precedence,9/10/99/100, beyondSAFE/u64,
equal/reverse/transitive cases, giant later components overridden by the major,
maximum decoded strings in each component and exact/+1 boundaries, escaped
digits, whitespace outside the JSON value, invalid grammar/types/UTF8 and
escaped duplicate properties rejected before shape checking. All218 original
Source cases remain regressions after the equivalent predicate extraction.
An actual App facade covers precedence, above-u64 values, escaped equality and
leading-zero refusal.

Ordering is supplied-value math only. It proves no actual OS installation/source
fact, current target qualification, trust, time or execution permission. No new
dependency, lock, Schema, CI, product version or gitlink is needed. Final evidence
appears in [implementation progress](upgrade-implementation-progress.md).
Full T27 integration remains unfinished. Downloads retain no total/cumulative
duration or retry cap.
