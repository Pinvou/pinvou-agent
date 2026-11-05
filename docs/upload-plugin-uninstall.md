# Uploaded plugin uninstall

Uploaded packages contain the user's only copy. Uninstall moves the whole
package from `bundles/<id>` to `marketplace/recycle-bin/<id>` and preserves the
existing transaction rollback and restore behavior.

On Windows, an MCP server whose working directory is inside a package can
prevent renaming that package with access denied (`os error 5`). Removing its
`mcp.json` entry does not stop an existing server. This is a reproduced cause;
the error alone does not distinguish it from external handles or directory ACLs.

The desktop command runs the filesystem transaction on its blocking worker:

1. Resolve the current source under the existing installation locks. Only the
   actual uploaded-package branch requests runtime preparation. An optional
   bundle mirror that is missing, unreadable or corrupt keeps its existing safe
   unregister-and-preserve-directory fallback for legacy MCP entries.
2. Acquire the transaction's complete leaf-file write set in sorted lock-path
   order before excluding engine use. A foreign process waiting on those files
   therefore cannot hold the runtime maintenance fence. Same-thread operations
   reuse the held locks, including compensation and rollback.
3. Close turn admission and exclude engine spawning and manual compaction.
   Refuse active/reserved sessions. Each engine must then accept retirement at
   its own idle boundary; unobserved autonomous turns, queued child results,
   goals, advisor work and live or unreported shell work prevent retirement.
   Uninstall does not cancel them to force the package move.
   Before committing that sealed boundary, recheck the application's lifecycle:
   a dequeued terminal can still be undergoing persistence or final delivery.
   Refusal resumes the same actor. A failed handshake is never approved merely
   because the producer already returned normally; restart clears that unknown
   consumer state when safe retirement cannot be established.
4. Reclaim only engines that have sealed that boundary, then wait for observed
   normal completion, including previously retiring engines. A queued Shutdown,
   abnormal completion or unknown completion is not a resource-release receipt.
5. Keep runtime and leaf-lock guards on the blocking worker's stack through
   filesystem commit or rollback. Release runtime exclusion before postcommit
   secret, consent and scope cleanup. The next conversation lazily rebuilds
   the engine from persisted history and current configuration.

Preparation is detached on caller drop so it cannot abandon an engine between
removal and shutdown. Blocking filesystem work also continues if the invoking
future is dropped. Retrying observes the resulting installed/recycled state.
Waiting for spawn exclusion, each idle request and completion receipts is
bounded to 15 seconds. A timed-out idle proposal cannot shut down the engine
later. Once an engine is removed, its reclaim is not abandoned on a timeout;
an unfinished receipt refuses the move and permits a later retry.

Standalone uploaded skills use the same protection. Preset uninstall and
connector disconnect keep their existing behavior. Existing eager companion
and OAuth cleanup ordering is retained, so a failed uninstall can still clear
that existing login state; package/configuration rollback is not a claim that
every prior side effect is reversed.

CodeWhale contains only reusable resource-lifetime primitives: join MCP boot
before closing transports, an owned completion receipt, and a process-local
two-phase idle retirement handshake on the engine actor. The host fences new
submissions. There is no new wire operation, protocol field, uninstall policy,
MCP manager or advisor state store in the foundation.

External processes, antivirus handles, detached descendants and directory ACLs
can still prevent moving a package. The existing filesystem error and rollback
remain authoritative; the app does not terminate other applications. The old
own-directory-missing skill candidate scan is unchanged by this fix and is not
covered by a claim that every historical skill-ownership race is repaired.

Verification uses a real Windows command, EnginePool and local stdio MCP with
a package working directory. It covers active refusal, foreign file locking,
caller cancellation, retry, rollback on corrupt recycle metadata, postcommit
cleanup waiting and next use; legacy optional-mirror failures are exercised.
Owning engine tests cover autonomous work, cancelled idle proposals, pending
boot, late host completion and completion lifetime. An app-owner regression
checks both active persistence and terminal-closing states against a real actor,
preserving its usability on refusal and allowing retirement after finalization.
UI recovery messages are provided in Chinese,
English and Japanese. No model or external service is needed.
