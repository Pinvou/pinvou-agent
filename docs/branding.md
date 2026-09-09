# 鲜小助 branding and compatibility

This independently maintained distribution is based on Pinvou Agent. Product-facing copy uses **鲜小助** in Simplified Chinese, English and Japanese, including application windows, native code sessions, review and voice prompts, bundled skills, knowledge services, exports and installation instructions.

The frontend display-name constant is `pinvou3-app/src/shared/brand.js`; localized copy consumes it through the `appTitle` entries in `src/shared/i18n/`. HTML startup titles and native packaging metadata also carry the display name because they are loaded before the frontend. Bundled prompt changes update the existing content hash, so the normal bundle extraction process refreshes installed resources.

## Compatibility boundaries

- Existing package/crate names, executables, application identifiers, environment variables, MCP tool names, URL schemes, database keys and `~/.pinvou3/` paths remain stable.
- The Linux Debian package retains the ASCII technical name `pinvou3`; its desktop launcher displays 鲜小助. macOS and Windows use 鲜小助 as the product name. Release scripts consume the new native bundle filenames while retaining existing downloadable artifact conventions.
- Existing conversations, user-provided assistant aliases, custom knowledge-server names, credentials and backups are not rewritten. The native code-agent display name is resolved from current branding instead of a saved legacy label.
- Third-party model names, speech transcription content, upstream repositories, copyright notices, licenses and trademark policies retain their original identities. Connector attribution identifiers and TLS certificate identities also remain stable.
- Existing upstream website, community, release and support links remain upstream links. The README identifies this project as an independent fork. Configure distribution-specific release/support infrastructure before publishing it as a separately supported product.

Native installers should be validated on their target operating systems before distribution, including upgrade from an existing installation. Source checks and UI builds do not replace native installation testing.
