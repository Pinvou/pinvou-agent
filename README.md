# Pinvou Agent

[English](README.md) | [简体中文](README.zh-CN.md)

**An open-source desktop AI agent that can use tools, work with files, build knowledge, and deliver real artifacts.**

[Website](https://pinvou.com/) · [CodeWhale engine](https://github.com/Pinvou/CodeWhale) · [Security](SECURITY.md) · [MIT License](LICENSE)

![Pinvou Agent desktop workspace](docs/assets/screenshots/home.webp)

Pinvou Agent brings models, tools, personal knowledge, specialist personas, and reusable workflows into one desktop workspace. It is designed for work that should end with a result—not just another chat response.

Use a local model for a private loop, connect any OpenAI-compatible endpoint, or extend the agent through MCP servers, CLI connectors, Skills, and workflows.

## What you can do

- **Turn conversations into deliverables** — create and edit Markdown, documents, spreadsheets, code, and other files, then preview every generated artifact in one place.
- **Work with your own context** — import PDF, Office, image, and text attachments; search local knowledge; and manage long-term memory explicitly.
- **Let the agent use tools** — connect MCP servers, local CLI tools, and external APIs from a unified tool store.
- **Reuse reliable methods** — package domain behavior as Skills, Commands, specialist personas, or multi-step workflows.
- **Choose how the agent acts** — use Plan mode for review-first work or YOLO mode for direct execution.
- **Run locally or in the cloud** — use local vLLM or any compatible model provider without locking the application to one vendor.

## A workspace built for outcomes

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

## Core capabilities

| Area | Included |
|---|---|
| Agent runtime | Streaming responses, tool loops, sessions, hooks, compaction, Skills, and MCP |
| Desktop workspace | Multi-session chat, attachment handling, artifact preview, and file editing |
| Knowledge | Local files, full-text and vector retrieval, memory review, and knowledge management |
| Automation | Scheduled tasks, reusable workflows, specialist personas, and structured deliverables |
| Model access | Local vLLM, built-in provider templates, and custom OpenAI-compatible endpoints |
| Extensibility | MCP servers, CLI connectors, APIs, Skills, Commands, and workflow packages |

> [!NOTE]
> Whether data leaves your machine depends on the model and tools you enable. A local model with local tools can stay local. Cloud models, remote MCP servers, and third-party connectors send the relevant requests to their respective services.

## Quick start

### Prerequisites

- Git with submodule support
- Node.js and npm
- A current Rust toolchain
- The [Tauri 2 system dependencies](https://v2.tauri.app/start/prerequisites/) for your platform
- An accessible OpenAI-compatible model endpoint

The source tree supports Linux, Windows, and macOS. The initial macOS target is Apple Silicon on macOS 14 or later.

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

### Connect a local model

Pinvou Agent supports local vLLM and custom OpenAI-compatible APIs. You can manage endpoints, models, and API keys from the application settings.

```bash
export DEEPSEEK_BASE_URL="http://127.0.0.1:8000/v1"
export DEEPSEEK_API_KEY="local-no-auth"
export DEEPSEEK_MODEL="your-model-name"
```

For a non-loopback plain HTTP endpoint in a trusted development network, explicitly set `DEEPSEEK_ALLOW_INSECURE_HTTP=1`.

## Architecture

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

[CodeWhale](https://github.com/Pinvou/CodeWhale) provides the agent engine: model calls, streaming, tool execution, sessions, MCP, Skills, hooks, and compaction. `pinvou3-app/` owns the desktop UI, runtime configuration, orchestration, and operating-system integration.

| Extension goal | Where it belongs |
|---|---|
| Add a domain agent or tool bundle | A `SKILL.md` package |
| Connect an external API | An independent MCP server or connector |
| Guide model behavior | Bundle instructions |
| Change desktop UI or system integration | `pinvou3-app/` |
| Fix a reusable engine issue | The CodeWhale fork, following the fork policy |

## Repository layout

```text
pinvou3-app/          Tauri 2 + React/Vite desktop application
CodeWhale/            Agent engine submodule
pinvou3-app/resources/mcp-servers/
                      Independent local MCP servers
workflows/            Reusable workflows
scripts/              Tests, guards, build, and release helpers
docs/                 Architecture and maintenance documentation
```

## Development checks

Run these commands from the repository root:

```bash
(cd pinvou3-app && npm run lint:ui)
(cd pinvou3-app && npm run build:ui)
(cd pinvou3-app && npm test)

(cd pinvou3-app/src-tauri && cargo test --lib -- --test-threads=1)

./scripts/fork-guard.sh --fast
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for the current contribution workflow and [docs/fork-policy.md](docs/fork-policy.md) for CodeWhale maintenance rules.

## Community and security

Pinvou Agent is licensed under the [MIT License](LICENSE). Please use [GitHub Issues](https://github.com/Pinvou/pinvou-agent/issues) for reproducible bugs and feature requests.

Do not report security vulnerabilities in public issues. Follow [SECURITY.md](SECURITY.md) or email `security@pinvou.com`.

---

Pinvou Agent is under active development. The `main` branch and the latest release notes are the source of truth for current behavior.
