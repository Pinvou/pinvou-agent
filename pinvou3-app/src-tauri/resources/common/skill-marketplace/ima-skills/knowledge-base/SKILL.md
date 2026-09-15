# IMA Knowledge Base

API base path: `openapi/wiki/v1`.

Use knowledge-base APIs when the user's target is a knowledge-base entry, folder, file, imported URL, or source media.

Do not use shell commands or environment inspection for credentials. Call the native `ima_openapi` tool; credentials are never tool arguments.

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

```json
{"api_path":"openapi/wiki/v1/search_knowledge_base","body":{"query":"","cursor":"","limit":20}}
```

```json
{"api_path":"openapi/wiki/v1/search_knowledge","body":{"query":"排期","knowledge_base_id":"<kb_id>","cursor":""}}
```

```json
{"api_path":"openapi/wiki/v1/import_urls","body":{"knowledge_base_id":"<kb_id>","urls":["https://example.com/article"]}}
```

## File Upload Guard

Local file upload is not an `ima_openapi` capability: only web page import via `import_urls` and collecting existing notes via `add_knowledge` are supported. Do not promise the user that local files can be uploaded; if the user asks to upload local files, explain this limitation instead.

## User-Facing Output

Hide internal IDs in normal answers. Show names, titles, summaries, paths, and concise progress. For failed business responses, show `msg` without credential or header details.
