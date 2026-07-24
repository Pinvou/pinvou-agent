# Third-Party Notices

Pinvou Agent includes or redistributes the following open-source components.
Their original licenses remain in effect.

| Component | Included form | License | Upstream |
|---|---|---|---|
| CodeWhale | Git submodule and linked Rust crates | MIT | https://github.com/Pinvou/CodeWhale |
| DingTalk Workspace CLI (`dws`) and skills | Apache-2.0 skill sources; Linux ARM64 CLI v1.0.51 downloaded during build | Apache-2.0 | https://github.com/DingTalk-Real-AI/dingtalk-workspace-cli |
| Lark CLI and skills | MIT skill sources; Linux ARM64 CLI v1.0.65 downloaded during build | MIT | https://github.com/larksuite/cli |
| WeCom CLI and skills | MIT skill sources; Linux ARM64 CLI v0.1.9 downloaded during build | MIT | https://github.com/WecomTeam/wecom-cli |
| SenseVoice.cpp | Built from pinned source on user setup; no executable stored in Git | MIT | https://github.com/lovemefan/SenseVoice.cpp |
| marked | Vendored browser script | MIT | https://github.com/markedjs/marked |
| DOMPurify | Vendored browser script | Apache-2.0 OR MPL-2.0 | https://github.com/cure53/DOMPurify |
| Tailwind CSS Play CDN runtime | Vendored browser script | MIT | https://github.com/tailwindlabs/tailwindcss |

Detailed license texts and upstream notices for bundled connectors are kept
next to their resources under `pinvou3-app/src-tauri/resources/`.

The exact connector URLs and SHA-256 checksums are recorded in
`pinvou3-app/src-tauri/resources/platforms/linux/aarch64/bundle/connectors/connectors.lock.json`.

Product names and trademarks belong to their respective owners. Inclusion
does not imply endorsement.
