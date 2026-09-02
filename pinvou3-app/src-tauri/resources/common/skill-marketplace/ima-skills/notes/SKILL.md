# IMA Notes

API base path: `openapi/note/v1`.

Use notes APIs when the user's real target is a note: search notes, list notebooks, list notes, read note content, create a new note, or append to an existing note.

Do not use shell commands or environment inspection for credentials. Call the native `ima_openapi` tool; credentials are never tool arguments.

## Operations

- Search notes by title: `search_note`
- Search notes by content: `search_note` with `search_type: 1`
- List notebooks: `list_notebook`
- List notes: `list_note`
- Read note content: `get_doc_content`
- Create note: `import_doc`
- Append to existing note: `append_doc`

## Write Rules

- If the user says "create/new note", use `import_doc`.
- If the user says "append/add to existing note", use `append_doc` only after the target note is clear.
- For vague requests like "record this" or "save to notes", ask whether to create a new note or append to an existing note.
- `append_doc` is irreversible in normal user flow. Ask before modifying when there is any ambiguity.
- Before `import_doc` or `append_doc`, ensure all string fields are UTF-8.
- Local image references are not supported in note content. Remove `file://`, absolute local paths, and Windows local image paths from Markdown, then tell the user which image links were omitted.

## Examples

```json
{"api_path":"openapi/note/v1/search_note","body":{"search_type":0,"query_info":{"title":"会议纪要"},"start":0,"end":20}}
```

```json
{"api_path":"openapi/note/v1/get_doc_content","body":{"note_id":"<note_id>","target_content_format":0}}
```

```json
{"api_path":"openapi/note/v1/import_doc","body":{"content_format":1,"content":"# 标题\n\n正文"}}
```

## Response Handling

IMA responses use `{ "code": 0, "msg": "...", "data": ... }`. Treat `code: 0` as success. For non-zero codes, show `msg` to the user and stop unless the next safe recovery step is obvious.

Do not expose `note_id`, `folder_id`, headers, Client ID, or API Key in normal user-facing answers.
