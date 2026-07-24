---
name: ima-skills
description: Tencent IMA OpenAPI skill for notes and knowledge-base operations. Use after the user connects IMA in Pinvou tool store.
version: 1.1.8-pinvou1
display_name: "腾讯 ima"
---

# 腾讯 ima OpenAPI

Use this skill when the user asks to search, read, create, append, upload, or organize content in Tencent IMA notes or IMA knowledge bases.

## Credential Rules

Pinvou stores IMA credentials in the local system credential store and injects them as environment variables when the connector is enabled.

Required environment variables:

- `IMA_CLIENT_ID`
- `IMA_API_KEY`

Do not ask the user to paste credentials into the chat. Do not write credentials to `~/.config/ima`, repository files, logs, notes, or artifacts.

Do not probe credentials with ad-hoc shell commands such as `echo $IMA_API_KEY`, `$env:IMA_CLIENT_ID`, `printenv`, or `env`. Pinvou injects IMA credentials only for the bundled helper process, so direct environment checks can report a false negative and must not be used to decide whether the connector is enabled.

To verify access or perform any IMA operation, call `ima_api.cjs` directly. If the helper exits non-zero with a missing-credential message, then tell the user to connect "腾讯 ima" from the Pinvou tool store.

## Module Routing

Read the relevant child instruction before operating:

- Notes: read `notes/SKILL.md` for note search, list, read, create, or append.
- Knowledge base: read `knowledge-base/SKILL.md` for knowledge-base search, browsing, upload, URL import, add note to knowledge base, or get media info.
- Cross-module tasks: read both child instructions before acting.

## API Helper

All calls go through the bundled helper:

```bash
node "$SKILL_DIR/ima_api.cjs" "openapi/check_skill_update" '{"version":"1.1.8"}'
```

In a shell, resolve `SKILL_DIR` to this skill directory before calling:

```bash
SKILL_DIR="$(pwd)"
node "$SKILL_DIR/ima_api.cjs" "openapi/wiki/v1/search_knowledge_base" '{"query":"","cursor":"","limit":20}'
```

The helper sends HTTP POST JSON requests only to `https://ima.qq.com` by default. It returns successful server responses on stdout. On program errors it exits non-zero and writes redacted JSON to stderr:

```json
{"code":-100,"msg":"..."}
```

Always parse the response JSON. IMA business success uses `code: 0`; for any non-zero business code, show the returned `msg` to the user without exposing credentials or internal headers.

## Safety

- Never expose `knowledge_base_id`, `media_id`, `folder_id`, `note_id`, Client ID, API Key, or HTTP headers unless the user explicitly needs a technical debug artifact and credentials are redacted.
- Ask before irreversible writes when the target note or knowledge base is ambiguous.
- For note writes, validate UTF-8 text and filter local image references.
- For knowledge-base file uploads, preserve the original file bytes and original filename.
