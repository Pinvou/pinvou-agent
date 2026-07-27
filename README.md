<div align="center">

<img src="pinvou3-app/src-tauri/icons/icon.png" alt="Pinvou Agent logo" width="120" />

# Pinvou Agent

**An open-source desktop AI agent that uses tools, works with your files, builds knowledge, and delivers real artifacts.**

[English](README.md) · [简体中文](README.zh-CN.md)

[![CI](https://github.com/Pinvou/pinvou-agent/actions/workflows/pr-check.yml/badge.svg)](https://github.com/Pinvou/pinvou-agent/actions/workflows/pr-check.yml)
[![License: MIT](https://img.shields.io/github/license/Pinvou/pinvou-agent)](LICENSE)
[![Version](https://img.shields.io/badge/dynamic/json?url=https%3A%2F%2Fraw.githubusercontent.com%2FPinvou%2Fpinvou-agent%2Fmain%2Fpinvou3-app%2Fpackage.json&query=%24.version&label=version&color=blue)](pinvou3-app/package.json)
[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux-lightgrey)](#-quick-start)
[![GitHub Stars](https://img.shields.io/github/stars/Pinvou/pinvou-agent?style=flat)](https://github.com/Pinvou/pinvou-agent/stargazers)

[Website](https://pinvou.com/) · [CodeWhale Engine](https://github.com/Pinvou/CodeWhale) · [Issues](https://github.com/Pinvou/pinvou-agent/issues) · [Discussions](https://github.com/Pinvou/pinvou-agent/discussions) · [Security](SECURITY.md)

<p align="center">
  <a href="https://pinvou.com/assets/videos/pinvou-agent-feature-update-2026-07.mp4">
    <img src="docs/assets/screenshots/home.webp" alt="Watch the 90-second Pinvou Agent feature demo">
  </a>
</p>
<p align="center">
  <a href="https://pinvou.com/assets/videos/pinvou-agent-feature-update-2026-07.mp4"><strong>▶ Watch the 90-second feature demo (Chinese)</strong></a>
</p>

<img src="docs/assets/screenshots/home.webp" alt="Pinvou Agent desktop workspace" />

</div>

Pinvou Agent is more than a chat window. It brings models, tools, personal knowledge, specialist personas, and reusable workflows into one desktop workspace — designed for work that should end with **a result**, not just another chat response.

Use a local model for a fully private loop, connect any OpenAI-compatible endpoint, and extend the agent with MCP servers, CLI connectors, Skills, and workflows.

## ✨ Features

### 🎯 From conversation to deliverables

- **Multi-session workspace** with title search — messages, tool calls, and artifacts persist with each session
- **Attachments** for PDF, Office documents, images, and text — drag, drop, or paste them in
- **Artifact panel** automatically collects every file the agent creates or edits; preview, locate, and open them in one place
- **Editable Markdown artifacts** — edit directly, or select a passage and ask the agent to revise it
- **Plan / YOLO modes** — review the plan first for complex work, or execute directly when the task is clear
- **Code sessions over ACP** (e.g. Codex) bring a coding agent into the same workspace

### 🧠 Knowledge and memory

- **Local knowledge base** with file management, full-text search, and vector retrieval
- **Memory center** captures long-term preferences and context, with explicit candidate review and confirmation
- **Persona card pool** — create, save, and apply specialist roles for different domains
- **Skills, Commands, and workflows** turn proven methods into stable, reusable capabilities

### 🔌 Real tools and connectors

- **Unified tool store** for local MCP servers, remote MCP servers, CLI tools, and API connectors
- **OAuth / SSO authorization** where supported — no manual key pasting
- Ready-made connectors for **Feishu (Lark), DingTalk, WeCom, Tencent Meeting, Tencent ima, Obsidian**, enterprise knowledge bases, and legal / enterprise data services
- **Remote control** — scan a QR code from your phone to view and steer the running workspace

### 🖥️ Built for daily operation

- **Local voice input** with on-demand speech model downloads
- **Centralized monitoring** of GPU, memory, disk, model service, and context usage
- **In-app updates** on Linux, with OTA delivery on macOS and Windows
- Sessions, settings, knowledge, and runtime extensions all live under `~/.pinvou3/`

> [!NOTE]
> Whether data leaves your machine depends on the model and tools you enable. A local model with local tools stays fully local. Cloud models, remote MCP servers, and third-party connectors send the relevant requests to their respective services.

## 📸 Screenshots

<table>
  <tr>
    <td width="50%"><img src="docs/assets/screenshots/tool-store.webp" alt="Pinvou Agent tool store"></td>
    <td width="50%"><img src="docs/assets/screenshots/artifacts-preview.webp" alt="Pinvou Agent artifact preview"></td>
  </tr>
  <tr>
    <td align="center">Extend the agent with tools and connectors</td>
    <td align="center">Preview and deliver generated artifacts</td>
  </tr>
</table>

## 🤖 Model Access

Pinvou Agent works with **local vLLM** and any **OpenAI-compatible API**. Save multiple model configurations in the app and switch between them per session. Built-in templates cover local vLLM, DeepSeek, Kimi, Qwen, Doubao, MiniMax, Zhipu, and MiMo — or fill in any custom compatible endpoint.

Local vLLM example:

```bash
export DEEPSEEK_BASE_URL="http://127.0.0.1:8000/v1"
export DEEPSEEK_API_KEY="local-no-auth"
export DEEPSEEK_MODEL="your-model-name"
```

Endpoints, model names, and API keys can also be managed directly in the application settings. For a non-loopback plain HTTP endpoint in a trusted development network, explicitly set `DEEPSEEK_ALLOW_INSECURE_HTTP=1`.

## 🚀 Quick Start

### Prerequisites

- Git with submodule support
- Node.js and npm
- A current Rust toolchain
- The [Tauri 2 system dependencies](https://v2.tauri.app/start/prerequisites/) for your platform
- An accessible OpenAI-compatible model endpoint

The source tree supports **Linux, Windows, and macOS**. The initial macOS target is Apple Silicon on macOS 14 or later.

### Run from source

```bash
git clone --recursive https://github.com/Pinvou/pinvou-agent.git
cd pinvou-agent/pinvou3-app
npm ci
npm run dev
```

If you cloned without submodules:

```bash
git submodule update --init --recursive
```

## 🏗️ Architecture

[![Tauri](https://img.shields.io/badge/Tauri-2.0-24C8D8?logo=tauri&logoColor=white)](https://v2.tauri.app/)
[![React](https://img.shields.io/badge/React-18-61DAFB?logo=react&logoColor=white)](https://react.dev/)
[![Vite](https://img.shields.io/badge/Vite-8-646CFF?logo=vite&logoColor=white)](https://vite.dev/)
[![Rust](https://img.shields.io/badge/Rust-stable-DEA584?logo=rust&logoColor=white)](https://www.rust-lang.org/)

```text
React + Vite UI
       ↕ Tauri commands / events
pinvou3-app (desktop orchestration)
       ↕ EngineHandle / AgentHarness
CodeWhale (agent engine submodule)
       ├─ OpenAI-compatible model services
       ├─ MCP servers and CLI connectors
       └─ Skills, Commands, Hooks, and Compaction
```

[CodeWhale](https://github.com/Pinvou/CodeWhale) provides the agent engine: model calls, streaming, tool execution, sessions, MCP, Skills, hooks, and compaction. `pinvou3-app/` owns the desktop UI, runtime configuration, orchestration, and operating-system integration — it never re-implements engine capabilities.

| Extension goal | Where it belongs |
|---|---|
| Add a domain agent or tool bundle | A `SKILL.md` package |
| Connect an external API | An independent MCP server or connector |
| Guide model behavior | Bundle instructions (`instructions.md`) |
| Change desktop UI or system integration | `pinvou3-app/` |
| Fix a reusable engine issue | The CodeWhale fork, following the [fork policy](docs/fork-policy.md) |

## 📁 Repository Layout

```text
pinvou3-app/          Tauri 2 + React/Vite desktop application
CodeWhale/            Agent engine submodule
pinvou3-app/resources/mcp-servers/
                      Independent local MCP servers
workflows/            Reusable workflows
scripts/              Tests, guards, build, and release helpers
docs/                 Architecture and maintenance documentation
```

## 🧪 Development Checks

Run these commands from the repository root:

```bash
(cd pinvou3-app && npm run lint:ui)
(cd pinvou3-app && npm run build:ui)
(cd pinvou3-app && npm test)

(cd pinvou3-app/src-tauri && cargo test --lib -- --test-threads=1)

./scripts/fork-guard.sh --fast
```

## 🤝 Contributing

Contributions are welcome! Please read [CONTRIBUTING.md](CONTRIBUTING.md) for the contribution workflow and CI gates, and [docs/fork-policy.md](docs/fork-policy.md) for CodeWhale maintenance rules. By participating, you agree to our [Code of Conduct](CODE_OF_CONDUCT.md).

## 💬 Community & Security

- 🐛 [GitHub Issues](https://github.com/Pinvou/pinvou-agent/issues) — reproducible bugs and focused feature requests
- 💡 [GitHub Discussions](https://github.com/Pinvou/pinvou-agent/discussions) — questions and ideas (community support is best-effort, see [SUPPORT.md](SUPPORT.md))
- 🔒 **Do not report security vulnerabilities in public issues** — use the private channel in [SECURITY.md](SECURITY.md) or email `security@pinvou.com`

Licensing, third-party attribution, SBOM, and brand-use boundaries are documented in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md), [docs/sbom.md](docs/sbom.md), and [TRADEMARKS.md](TRADEMARKS.md).

## ⭐ Star History

[![Star History Chart](https://api.star-history.com/svg?repos=Pinvou/pinvou-agent&type=Date)](https://star-history.com/#Pinvou/pinvou-agent&Date)

---

<div align="center">

Pinvou Agent is under active development — the `main` branch and the latest release notes are the source of truth for current behavior.

**[MIT License](LICENSE)** · Made with ❤️ by the Pinvou team and contributors

</div>
