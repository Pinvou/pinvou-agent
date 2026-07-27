---
name: ima-skills
description: Tencent IMA OpenAPI skill for notes and knowledge-base operations. Use after the user connects IMA in Pinvou tool store.
version: 1.1.8-pinvou2
display_name: "腾讯 ima"
---

# 腾讯 ima OpenAPI

Use this skill when the user asks to search, read, create, append, upload, or organize content in Tencent IMA notes or IMA knowledge bases.

## Credential Rules

Pinvou stores IMA credentials in the local system credential store. The native `ima_openapi` tool reads them only when it sends a request to the fixed official endpoint.

Do not ask the user to paste credentials into the chat. Do not write credentials to `~/.config/ima`, repository files, logs, notes, or artifacts.

Do not probe credentials with shell commands, environment inspection, local files, or ad-hoc network requests. Never pass a host, URL, Client ID, API Key, or HTTP header as tool input.

To verify access or perform any IMA operation, call `ima_openapi`. If it reports missing credentials, tell the user to connect "腾讯 ima" from the Pinvou tool store.

## Module Routing

Read the relevant child instruction before operating:

- Notes: read `notes/SKILL.md` for note search, list, read, create, or append.
- Knowledge base: read `knowledge-base/SKILL.md` for knowledge-base search, browsing, upload, URL import, add note to knowledge base, or get media info.
- Cross-module tasks: read both child instructions before acting.

## Native Tool

All calls go through Pinvou's native `ima_openapi` tool. Pass only an allowlisted `api_path` and a JSON object in `body`:

```json
{
  "api_path": "openapi/wiki/v1/search_knowledge_base",
  "body": {"query": "", "cursor": "", "limit": 20}
}
```

The tool sends POST JSON requests only to `https://ima.qq.com`, applies a response-size limit, and does not expose credentials to the model or subprocesses.

Always parse the response JSON. IMA business success uses `code: 0`; for any non-zero business code, show the returned `msg` to the user without exposing credentials or internal headers.

## Safety

- Never expose `knowledge_base_id`, `media_id`, `folder_id`, `note_id`, Client ID, API Key, or HTTP headers unless the user explicitly needs a technical debug artifact and credentials are redacted.
- Ask before irreversible writes when the target note or knowledge base is ambiguous.
- For note writes, validate UTF-8 text and filter local image references.
- For knowledge-base file uploads, preserve the original file bytes and original filename.
