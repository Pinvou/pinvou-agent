# WebUI Relay protocol v2

The Relay connects one persistent desktop endpoint to at most one active WebUI
client. It authenticates peers and forwards a small, typed JSON protocol. It does
not interpret RPC commands, store conversation data, assign stream sequence
numbers, or maintain the replay journal.

All protocol messages are UTF-8 JSON objects and must contain the exact numeric
field `v: 2`. This applies both before and after authentication, including revoke
messages. The Relay rejects and closes a socket when the field is missing or has
any other value; it never upgrades an older message by rewriting its version.
Because JSON parsing collapses repeated keys to the last occurrence, a frame
whose text repeats the `v` key in any object (including escaped spellings such
as `"\u0076"`) is rejected as `bad_json` before parsing instead of being
upgraded to whatever the last occurrence says. A frame that nests any value
more than 128 levels deep is likewise rejected as `bad_json`; senders must keep
nesting within this bound. serde_json parses chains of at most 127 containers
across the whole frame, envelope included, so the cap deliberately sits above
anything the desktop could deserialize: no parseable frame is rejected, while
unbounded-depth frames are still caught before JSON.parse.
Credentials belong only in the authentication messages. The browser link keeps
its credentials in the URL fragment, which is never included in an HTTP request.
On the wire, `access_token` is the only accepted access-token field; legacy
top-level aliases such as `token` and `pairing_token` are explicitly rejected.

## Desktop registration

```json
{
  "v": 2,
  "type": "desktop_endpoint_register",
  "endpoint_id": "endpoint_...",
  "access_token": "...",
  "desktop_secret": "..."
}
```

The first registration creates an in-memory endpoint unless its `endpoint_id`
has a persistent revoke tombstone. Later registrations must match both
credential hashes. A newer authenticated desktop socket replaces the old socket
without changing the endpoint or Web lease. A tombstoned ID is rejected with
`endpoint_revoked`, even when the caller still holds the old credentials.

When a newer registration replaces a still-connected desktop socket, the Relay
sends the old socket a `desktop_endpoint_replaced` notification and closes it
with WebSocket close code 4001. The endpoint identity, its credential hashes,
and any active Web lease are unchanged.

```json
{
  "v": 2,
  "type": "desktop_endpoint_registered",
  "endpoint_id": "endpoint_...",
  "web_client_connected": false,
  "lease_id": null
}
```

The endpoint remains available while its desktop is connected and for
`ENDPOINT_OFFLINE_TTL_MS` after the desktop disconnects (24 hours by default).
Reconnecting within that interval preserves the Web lease. When the TTL expires,
the Relay removes the in-memory endpoint, notifies and closes any Web client with
reason `desktop_offline_ttl_expired`, and releases its capacity. TTL expiry is
not a revoke: the desktop may later recreate the same endpoint with its
persistent credentials, at which point a browser must join again.

Live endpoints are not persisted. After a Relay restart, a non-revoked desktop
recreates its endpoint with the same credentials. Revoked endpoint IDs are the
exception: their credential-free tombstones survive restart and prevent
re-registration.

## Web client join and lease takeover

```json
{
  "v": 2,
  "type": "web_client_join",
  "endpoint_id": "endpoint_...",
  "access_token": "...",
  "stream_epoch": "last-seen-epoch",
  "after_seq": 42
}
```

On success, the Relay assigns an unguessable lease:

```json
{
  "v": 2,
  "type": "web_client_joined",
  "endpoint_id": "endpoint_...",
  "lease_id": "lease_...",
  "desktop_connected": true
}
```

Only one Web lease is active per endpoint. A newer client receives a new lease;
the previous client receives `endpoint_replaced` and is closed. The Relay sends
the desktop a `web_client_connected` status containing the active lease and the
join cursor. The desktop owns replay and decides whether to replay journaled
events, send `stream_reset`, or send `desktop_snapshot`.

## Forwarded messages

After authentication, the Relay accepts only these Web-to-desktop types:

- `rpc_request`
- `event_subscribe`
- `event_unsubscribe`
- `client_ready`

It accepts only these desktop-to-Web types:

- `rpc_response`
- `event`
- `stream_reset`
- `desktop_snapshot`

The original top-level fields are forwarded unchanged except that the Relay
sets authoritative `v`, `endpoint_id`, and `lease_id` fields. Credential fields
(`access_token`, `token`, `pairing_token`, `desktop_secret`) are additionally
stripped from forwarded frames. A desktop message that explicitly targets a
stale lease is dropped so a late response cannot leak into the replacement
client.

The desktop is responsible for command/event policy validation, idempotency via
`client_request_id`, `stream_epoch`, monotonic `seq`, and its bounded replay
journal. The Relay holds none of that business state.

`client_ready` is a two-phase handshake between the shared UI and desktop:

1. The initial `client_ready` carries `state_ready: false`. It negotiates
   capabilities and establishes the replay cursor, but the desktop must not
   deliver business events yet.
2. After the shared UI durably hydrates its local state, it sends
   `client_ready` with `state_ready: true`. Only then may the desktop replay the
   ready-window events after the negotiated cursor and begin live delivery.

Within the forwarded `event` envelope, the authoritative chat-turn allowlist
includes both `chat:user_message` (including `base_transcript_revision`) and
`chat:transcript_committed`. They are authoritative round events, not local UI
hints. The Relay forwards the typed envelope without interpreting either event.

## Connection status and revoke

When the desktop disconnects, the active Web client receives:

```json
{
  "v": 2,
  "type": "desktop_connection_state",
  "endpoint_id": "endpoint_...",
  "lease_id": "lease_...",
  "status": "offline"
}
```

The same message with `status: "connected"` is sent after desktop reconnection.
The desktop receives `web_client_connected` and `web_client_disconnected` with
the corresponding lease.

The desktop invalidates a link with:

```json
{
  "v": 2,
  "type": "desktop_endpoint_revoke",
  "endpoint_id": "endpoint_...",
  "desktop_secret": "...",
  "reason": "refreshed"
}
```

The Relay first writes the endpoint tombstone atomically to
`PINVOU_REMOTE_STATE_PATH`, fsyncs the temporary file, and renames it into place.
Only after that succeeds does the desktop receive `desktop_endpoint_revoked`.
The Web client then receives `endpoint_revoked` and is closed. A later Web join
fails with `endpoint_not_found`; an old desktop registration fails with
`endpoint_revoked`. A requester revoking a different endpoint with that
endpoint's desktop secret keeps its own session and receives
`desktop_endpoint_revoked` instead of being closed. If persistence fails, the Relay sends
`revoke_persistence_failed`, closes the requester, and leaves the endpoint
usable rather than acknowledging a revoke it cannot preserve. A revoke that
hits the tombstone capacity is refused before any write with
`revoked_endpoint_capacity_reached`; the endpoint stays usable until the file
is pruned or a limit is raised — a larger `MAX_RELAY_STATE_BYTES` raises the
derived ceiling, and a larger `MAX_REVOKED_ENDPOINTS` raises it when the
configured value, not the state-size clamp, is the binding limit.

The state file contains only schema version, endpoint IDs, and revoke times; it
does not contain access tokens, desktop secrets, or their hashes. New revocations
are bounded by `MAX_REVOKED_ENDPOINTS`, which is clamped to what
`MAX_RELAY_STATE_BYTES` can hold; a state file holding more tombstones than
today's derivation still loads in full and every loaded tombstone stays
enforced. Back up the state file with the deployment's durable data. Deleting it
deliberately re-enables old endpoint IDs. A state file that is not a regular
file, that exceeds `MAX_RELAY_STATE_BYTES`, or that is not valid schema-v1 JSON
aborts startup with a fixed `[relay-error]` line.

## Error codes

The Relay reports rejects with a `type: "error"` message carrying a `code`.
Codes enforced by the reject helper also close the socket with WebSocket close
code 1008. Five informational codes leave the connection open:
`bad_json`, `bad_message`, `binary_unsupported`, `unsupported_message_type`,
and `desktop_offline`. Graceful closes wait for the peer's close handshake so
queued messages are delivered, and frames that arrive after the close are
ignored; a peer that withholds the handshake is terminated unconditionally
after 5 seconds, so the drain cannot outlive ws's own 30-second close timeout.

| Code | Trigger |
| --- | --- |
| `bad_json` | A text frame is not valid JSON, repeats the `v` key in any object, or nests any value more than 128 levels deep. |
| `bad_message` | The parsed frame is not a JSON object. |
| `binary_unsupported` | A binary WebSocket frame arrived. |
| `unsupported_protocol_version` | `v` is missing or is not `2`. |
| `legacy_token_alias_unsupported` | The message carries a legacy top-level `token` or `pairing_token` field. |
| `authentication_required` | A non-handshake message arrived before authentication completed. |
| `authentication_timeout` | The socket did not authenticate within the deadline. |
| `bad_desktop_endpoint_register` | Malformed `desktop_endpoint_register`: invalid `endpoint_id`, missing or oversized credentials, or the socket already authenticated. |
| `endpoint_capacity_reached` | Creating a new endpoint would exceed total endpoint capacity. |
| `endpoint_creation_rate_limited` | The per-IP endpoint creation rate was exceeded. |
| `invalid_access_token` | Desktop re-registration whose access token hash does not match the endpoint. |
| `invalid_desktop_secret` | Desktop re-registration or revoke whose desktop secret hash does not match the endpoint. |
| `invalid_token` | `web_client_join` whose access token hash does not match the endpoint. |
| `bad_web_client_join` | Malformed `web_client_join`: invalid `endpoint_id`, missing access token, or an already-authenticated socket. |
| `endpoint_not_found` | Join, revoke, or forwarded message referenced an endpoint that does not exist. |
| `stale_lease` | A Web message arrived from a socket that no longer holds the active lease. |
| `desktop_endpoint_replaced` | A desktop message arrived from a socket already replaced by a newer registration. |
| `unsupported_message_type` | The authenticated message type is not in that direction's allowlist. |
| `desktop_offline` | A Web-to-desktop message arrived while the desktop socket is not connected. |
| `ingress_rate_limited` | The per-connection ingress message or byte rate was exceeded. |
| `endpoint_revoked` | Desktop registration for a tombstoned endpoint ID. The same name is also the message type delivered to the Web client on revoke or TTL expiry, with `reason` such as `desktop_offline_ttl_expired`. |
| `revocation_in_progress` | A second revoke arrived while one is already in progress for the endpoint. |
| `revoke_persistence_failed` | Persisting the revoke tombstone failed; the requester is closed and the endpoint remains usable. |
| `revoked_endpoint_capacity_reached` | The tombstone capacity is exhausted (nothing was written); the requester is closed and the endpoint remains usable. |
| `revoke_failed` | An unexpected error occurred while processing a revoke. |

## HTTP and operational behavior

- `GET /pinvou3/remote/healthz` exposes aggregate counters only.
- The startup banner intentionally omits the bind address and reports the
  effective limits, including the silently-derived ones: the post-clamp
  `revoked_capacity`, the effective `heartbeat` interval, and the
  `max_relay_state` cap. The listening socket is configured entirely by
  `PORT` and `PINVOU_REMOTE_HOST`.
- `/pinvou3/remote/` serves `web/dist/index.html`; extensionless paths use the
  same SPA fallback.
- Hashed `assets/` files are immutable. HTML is never cached and fixed-name
  scripts must be revalidated so a WebUI update takes effect.
- WebSocket authentication has a deadline. The server also enforces total
  socket capacity, endpoint capacity and creation rate, per-IP connection rate,
  per-connection ingress message and byte rates, maximum message size, proxy
  allowlists, and ping/pong liveness.
- Proxy allowlist entries must be literal IP addresses. Any other value in
  `PINVOU_REMOTE_ALLOWED_PROXY_IPS` or `PINVOU_REMOTE_TRUSTED_PROXY_IPS` aborts
  startup with `proxy_ip_configuration_invalid` instead of being ignored; a
  value with content that contains no address (for example `,,,`) is also
  rejected. Entries are canonicalized before matching: lowercased, IPv6
  expanded to its full form, and IPv4-mapped spellings reduced to the embedded
  IPv4, so any valid spelling of an address matches regardless of the form the
  runtime reports a peer in. A zone-id entry such as `fe80::1%eth0` matches
  only a peer whose reported address carries the identical suffix, compared
  verbatim — the suffix is the relay host's own interface name for that
  connection, so use the spelling the runtime reports; an IPv4-mapped
  spelling never carries a zone. A blank value means an
  empty set: an empty
  `PINVOU_REMOTE_ALLOWED_PROXY_IPS` admits every source, and an empty
  `PINVOU_REMOTE_TRUSTED_PROXY_IPS` trusts no proxy. A direct peer on
  `127.0.0.1` or `::1` is always admitted, whatever the list says. Only when
  `PINVOU_REMOTE_TRUSTED_PROXY_IPS` is unset does it fall back to
  `PINVOU_REMOTE_ALLOWED_PROXY_IPS`. For rate limiting, the client address is
  the rightmost `X-Forwarded-For` item when the direct peer is a trusted proxy;
  `X-Real-IP` is consulted only when `X-Forwarded-For` is entirely absent, and
  a present-but-unusable `X-Real-IP` falls back to the direct peer too. The
  forwarded header falls back to the direct peer address when its rightmost
  item is empty or not a literal IP of at most 128 characters (an entirely
  empty header included), or when the whole header exceeds 4096 characters;
  there is no separate "malformed header" fallback. Addresses are compared in
  canonical form (lowercase, IPv6 expanded, IPv4-mapped spellings reduced), so
  header spellings and configuration entries match across forms. Repeated
  `X-Forwarded-For` lines are joined into one list by the runtime, so the
  rightmost item of the joined list is used; they are not a fallback case. A
  trusted proxy must set these headers itself and strip client-supplied ones;
  a proxy that forwards them lets senders pick their rate-limit identity.
- The relay listens on `PINVOU_REMOTE_HOST`, by default `0.0.0.0` — every
  IPv4 interface (set `::` to include IPv6), with endpoint registration open
  to whichever sources the
  allowlist admits. Set it to `127.0.0.1` to keep the relay local; the
  startup banner reports no address, so check the deployment configuration
  rather than the logs.
- Every startup failure exits with code 1 and one fixed `[relay-error]` line:
  configuration problems (`proxy_ip_configuration_invalid` naming the
  variable, an invalid `PORT`, invalid browser origin entries), state-file
  problems (`relay_state_load_failed ...`, the over-size line, or the
  non-regular-file line), and bind failures (`listen_failed code=<errno>`).
  No startup report echoes a filesystem path, the offending value, or the
  bind address. Three runtime events share the same fixed-line discipline and
  leave the process up: `websocket_protocol_violation code=<ws error code>`
  (a contained per-socket protocol violation), `revocation_persist_failed
  code=<errno>`, and `revocation_unexpected_failure`.
- After the handlers are installed at the very end of startup, `SIGINT` and
  `SIGTERM` terminate all WebSocket clients without a close handshake (peers
  observe an abnormal closure) and exit the process with code 0; in-flight
  HTTP responses are not drained. A signal that arrives earlier — during
  startup — takes the default action instead.
- The Relay rejects any frame whose complete serialized WebSocket message
  exceeds `MAX_PAYLOAD_BYTES` (protocol v2 requires at least 4 MiB), counting
  the whole envelope rather than just the event payload. The desktop keeps its
  own frames at or below `4 MiB - 64 KiB` so they fit inside that bound.
  Scan and parse costs are linear in frame size, so raising
  `MAX_PAYLOAD_BYTES` above the 4 MiB default trades per-frame event-loop
  latency: a maximal frame briefly blocks the Relay. Values above 16 MiB
  clamp to a 16 MiB ceiling.
- The desktop replay journal retains at most 1,024 complete event frames and
  16 MiB. When either boundary evicts the requested cursor, the desktop rotates
  the stream epoch and sends `stream_reset`; the Relay never fabricates a gap.
- Every outbound send checks `ws.bufferedAmount` plus the next encoded frame
  against `WS_MAX_BUFFERED_BYTES`. A slow recipient is terminated when the
  high-water mark would be crossed; the Relay does not accumulate an unbounded
  per-client queue.
- Static file serving reads each asset fully into memory, for `HEAD` as well
  as `GET` (so the two can never disagree), and the web root is
  deployment-controlled: keep large files out of it.
- Browser WebSocket upgrades must use either a same-host `Origin` or an origin
  listed in `PINVOU_REMOTE_ALLOWED_WEB_ORIGINS`. Native desktop clients may omit
  `Origin`. The check assumes the reverse proxy preserves the public `Host` and
  is defense in depth against cross-site WebSocket hijacking.
- Access tokens and desktop secrets are retained only as SHA-256 hashes and are
  compared with constant-time equality.

Relevant limits are configured with `ENDPOINT_OFFLINE_TTL_MS`,
`WS_INGRESS_WINDOW_MS`, `WS_INGRESS_MESSAGE_LIMIT`,
`WS_INGRESS_BYTE_LIMIT`, `MAX_PAYLOAD_BYTES`, `WS_MAX_BUFFERED_BYTES`,
`MAX_REVOKED_ENDPOINTS`, and `MAX_RELAY_STATE_BYTES`; connection capacity and
pacing use `MAX_ENDPOINTS`, `MAX_WS_CONNECTIONS`, `WS_CONNECT_LIMIT` /
`WS_CONNECT_WINDOW_MS`, `ENDPOINT_CREATE_LIMIT` / `ENDPOINT_CREATE_WINDOW_MS`,
`WS_AUTH_TIMEOUT_MS`, and `HEARTBEAT_INTERVAL_MS` (fallback 15 s, floor 5 s,
ceiling 24 h). Relay tests always point
`PINVOU_REMOTE_STATE_PATH` at a temporary directory.

Other knobs: `PORT` (default 8787), `PINVOU_REMOTE_HOST` (default `0.0.0.0`,
every IPv4 interface; `::` for IPv6), `PINVOU_REMOTE_PUBLIC_BASE_PATH`
(default `/pinvou3/remote`), `PINVOU_REMOTE_WEB_DIR` (default
`<relay dir>/web/dist`, a deployment-time build location that may not exist),
and `PINVOU_REMOTE_ALLOWED_WEB_ORIGINS`. The legacy spellings `MAX_ROOMS`,
`ROOM_CREATE_LIMIT`, and `ROOM_CREATE_WINDOW_MS` still map onto
`MAX_ENDPOINTS`, `ENDPOINT_CREATE_LIMIT`, and `ENDPOINT_CREATE_WINDOW_MS`.

Origin checks and caller-supplied first-registration credentials are not a
production enrollment system. Formal desktop enrollment remains a deployment
gate and must be provided before exposing unrestricted endpoint registration to
an untrusted network; protocol v2 intentionally does not design or enable that
registration authority.
