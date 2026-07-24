# WebUI Relay protocol v2

The Relay connects one persistent desktop endpoint to at most one active WebUI
client. It authenticates peers and forwards a small, typed JSON protocol. It does
not interpret RPC commands, store conversation data, assign stream sequence
numbers, or maintain the replay journal.

All protocol messages are UTF-8 JSON objects and must contain the exact numeric
field `v: 2`. This applies both before and after authentication, including revoke
messages. The Relay rejects and closes a socket when the field is missing or has
any other value; it never upgrades an older message by rewriting its version.
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
sets authoritative `v`, `endpoint_id`, and `lease_id` fields. A desktop message
that explicitly targets a stale lease is dropped so a late response cannot leak
into the replacement client.

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
`endpoint_revoked`. If persistence fails, the Relay sends
`revoke_persistence_failed`, closes the requester, and leaves the endpoint
usable rather than acknowledging a revoke it cannot preserve.

The state file contains only schema version, endpoint IDs, and revoke times; it
does not contain access tokens, desktop secrets, or their hashes. Tombstones are
bounded by `MAX_REVOKED_ENDPOINTS` and `MAX_RELAY_STATE_BYTES`. Back up the state
file with the deployment's durable data. Deleting it deliberately re-enables old
endpoint IDs.

## HTTP and operational behavior

- `GET /pinvou3/remote/healthz` exposes aggregate counters only.
- `/pinvou3/remote/` serves `web/dist/index.html`; extensionless paths use the
  same SPA fallback.
- Hashed `assets/` files are immutable. HTML is never cached and fixed-name
  scripts must be revalidated so a WebUI update takes effect.
- WebSocket authentication has a deadline. The server also enforces total
  socket capacity, endpoint capacity and creation rate, per-IP connection rate,
  per-connection ingress message and byte rates, maximum message size, proxy
  allowlists, and ping/pong liveness.
- Protocol v2 requires `MAX_PAYLOAD_BYTES` to be at least 4 MiB. Desktop-originated
  frames are bounded by the complete serialized WebSocket message, not just the
  event payload, and stay at or below `4 MiB - 64 KiB`.
- The desktop replay journal retains at most 1,024 complete event frames and
  16 MiB. When either boundary evicts the requested cursor, the desktop rotates
  the stream epoch and sends `stream_reset`; the Relay never fabricates a gap.
- Every outbound send checks `ws.bufferedAmount` plus the next encoded frame
  against `WS_MAX_BUFFERED_BYTES`. A slow recipient is terminated when the
  high-water mark would be crossed; the Relay does not accumulate an unbounded
  per-client queue.
- Browser WebSocket upgrades must use either a same-host `Origin` or an origin
  listed in `PINVOU_REMOTE_ALLOWED_WEB_ORIGINS`. Native desktop clients may omit
  `Origin`. The check assumes the reverse proxy preserves the public `Host` and
  is defense in depth against cross-site WebSocket hijacking.
- Access tokens and desktop secrets are retained only as SHA-256 hashes and are
  compared with constant-time equality.

Relevant limits are configured with `ENDPOINT_OFFLINE_TTL_MS`,
`WS_INGRESS_WINDOW_MS`, `WS_INGRESS_MESSAGE_LIMIT`,
`WS_INGRESS_BYTE_LIMIT`, `MAX_PAYLOAD_BYTES`, `WS_MAX_BUFFERED_BYTES`,
`MAX_REVOKED_ENDPOINTS`, and `MAX_RELAY_STATE_BYTES`. Relay tests always point
`PINVOU_REMOTE_STATE_PATH` at a temporary directory.

Origin checks and caller-supplied first-registration credentials are not a
production enrollment system. Formal desktop enrollment remains a deployment
gate and must be provided before exposing unrestricted endpoint registration to
an untrusted network; protocol v2 intentionally does not design or enable that
registration authority.
