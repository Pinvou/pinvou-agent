# Scheduled Session Token Accounting (EngineCumulative)

> English mirror of [`scheduled-token-accounting.md`](./scheduled-token-accounting.md); the Chinese original is authoritative on any divergence. Added per `CONTRIBUTING.md`'s English requirement for new developer documentation.

The 2026-11-12 alignment with the official build (upstream commit `624c384941`) took only the accounting kernel on the sessions side: the `ScheduledTokenAccounting::EngineCumulative { base_total_tokens, engine_total_tokens }` variant plus the paired match in `persist_scheduled_engine_state`. The lifetime total goes through `scheduled_lifetime_total` (saturating base + engine); `persist_scheduled_engine_state` and `persist_scheduled_token_total` share that single encoding, and the combined result replaces rather than accumulates into the persisted total.

The producer is deliberately unwired on both sides: neither this tree nor the official build constructs the variant outside tests, and no PR in the split series (#856–#868) wires one. Wiring needs only an app-side follow-up PR — the spawn-time base is already read from the persisted total in `assistant/engine.rs` on every spawn, cross-turn usage is already accumulated in the forwarder's `TurnComplete` arm, and the producer only needs to construct `EngineCumulative` in the `SessionUpdated` arm.

The workspace-field half-hunk in the same official-build alignment batch (excluded from the public alignment commit `624c384941`) was judged broken and is deliberately not ported: `session.metadata.workspace` keeps its existing profile-derived semantics.

Accounting contract: `engine_total_tokens` is usage accumulated since that engine spawned, never a per-update delta; the combined result replaces the persisted total, and a stale base may lower it, so every re-spawn must reload the base from the persisted total. The saturating clamp and the replace-not-max() semantics are both pinned by a regression test (base + engine saturates at `u64::MAX`; stale base `{100, 5}` → 105):

```sh
cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml --lib scheduled_engine
```
