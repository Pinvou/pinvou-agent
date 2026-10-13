# API object read-set validation compatibility

T27 batch 27 repairs the reference API adapter required before client execution
review response development. A valid two-member currentReadSet passes the frozen
Schema on ordinary JSON data, but assertApiContract's strict reparse creates
null-prototype objects. Ajv's uniqueItems implementation calls fast-deep-equal,
which assumes ordinary object valueOf and throws an unstructured TypeError.

The adapter now canonicalizes once and applies the unchanged strict parseJson
to those exact bytes, including UTF-8, Unicode, number, duplicate, depth, string,
member and aggregate bounds. Only Schema validation gets an ordinary JSON.parse
copy of those already validated canonical bytes. Original data still drives all
existing semantic checks and is returned by identity. No properties are assigned
or merged; JSON.parse creates own data members for prototype-like spellings and
cannot invoke caller hooks. The original strict parser and its null prototypes
remain in place. Invalid extra fields fail the closed Schema.

This corrects accepted valid object read sets to the already frozen Schema.
uniqueItems still compares entire JSON objects: reordered duplicates and zero/
negative-zero equivalent references fail, while distinct whole references pass.
The API contract does not prove a complete/current read set or real review,
consumption, object verification or execution rights. Those are owner/transport
and T19/T32/T36 responsibilities.

Five regression groups cover all three execution-review operations, return
identity, unchanged strict input prototypes/bytes, duplicate/distinct object
equality, prototype-like fields/no pollution, uninvoked getters/toJSON and strict
resource limits. Before the production correction, three groups failed with the
original TypeError and two guards passed; after correction all five passed.

No Schema, OpenAPI, contract/version profile, fixed vectors, crypto, trust policy,
resource capacity, locks, product version or gitlinks change. The service pinned
artifact does not copy this reference implementation. Full reference package,
affected Rust API oracle groups and snapshot/architecture checks provide the
compatibility evidence recorded in [progress](upgrade-implementation-progress.md).
Downloads retain no total/cumulative duration or retry cap.
