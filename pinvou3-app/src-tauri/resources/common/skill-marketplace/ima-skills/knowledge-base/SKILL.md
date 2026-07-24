# IMA Knowledge Base

API base path: `openapi/wiki/v1`.

Use knowledge-base APIs when the user's target is a knowledge-base entry, folder, file, imported URL, or source media.

Do not run separate shell commands to inspect `IMA_CLIENT_ID` or `IMA_API_KEY`. Call `ima_api.cjs` directly; the helper is the only reliable credential check inside Pinvou.

## Operations

- Search knowledge bases: `search_knowledge_base`
- Get knowledge-base details: `get_knowledge_base`
- Browse knowledge-base contents: `get_knowledge_list`
- Search within a knowledge base: `search_knowledge`
- List addable knowledge bases: `get_addable_knowledge_base_list`
- Import web URLs: `import_urls`
- Add an existing IMA note to a knowledge base: `add_knowledge` with `media_type: 11`
- Get media source info: `get_media_info`

## Routing Rules

- If the user names a target knowledge base, search by name with `search_knowledge_base`; do not use `get_addable_knowledge_base_list` first.
- If the user wants to add content but does not specify a knowledge base, call `get_addable_knowledge_base_list` and ask them to choose.
- Root folder operations omit `folder_id`; never pass `knowledge_base_id` as `folder_id`.
- If `get_media_info` indicates `media_type: 11`, switch to the notes module and call `get_doc_content` with the note ID from the response.

## Examples

```bash
node "$SKILL_DIR/ima_api.cjs" "openapi/wiki/v1/search_knowledge_base" '{"query":"","cursor":"","limit":20}'
```

```bash
node "$SKILL_DIR/ima_api.cjs" "openapi/wiki/v1/search_knowledge" '{"query":"排期","knowledge_base_id":"<kb_id>","cursor":""}'
```

```bash
node "$SKILL_DIR/ima_api.cjs" "openapi/wiki/v1/import_urls" '{"knowledge_base_id":"<kb_id>","urls":["https://example.com/article"]}'
```

## File Upload Guard

Only upload files when the file type is supported by IMA OpenAPI and the user has clearly chosen the destination knowledge base. Preserve original file bytes and original filename. If duplicate names are detected, ask whether to keep both with a timestamped filename or cancel; replacing is not supported.

## User-Facing Output

Hide internal IDs in normal answers. Show names, titles, summaries, paths, and concise progress. For failed business responses, show `msg` without credential or header details.
