# brain-mcp

[![CI](https://github.com/dck/brain-mcp/actions/workflows/ci.yml/badge.svg)](https://github.com/dck/brain-mcp/actions/workflows/ci.yml)

MCP server that gives AI coding agents persistent, cross-project memory backed by an Obsidian-compatible markdown vault with semantic vector search.

## Why

I use Claude Code across dozens of projects daily. Every new session starts from zero. I re-explain the same deployment procedures, re-describe the same architectural decisions, re-debug the same issues I solved last week. The agent forgets everything the moment the session ends.

I built brain-mcp because I wanted my AI coding assistant to actually *remember* things — not just within a project, but across all of them. When I debug a tricky Terraform state issue on Monday, I want the agent to recall that insight on Friday when I hit a similar problem in a different repo. When I document a deployment runbook, I want every future session to find it automatically.

The memories live as plain markdown files in an Obsidian vault, so I can browse, edit, and link them with Obsidian's graph view. The agent never knows about files or embeddings — it just calls `memory_store` and `memory_search`. Everything else happens underneath.

## How it works

```
Claude Code Session 1 ──┐
Claude Code Session 2 ──┼── stdio bridges ──> brain-mcp HTTP server ──> vault + index
Claude Code Session 3 ──┘        (one per session)       (singleton)
```

- **Singleton server**: one process per machine, shared across all Claude Code sessions
- **stdio bridge**: Claude Code spawns `brain-mcp serve --stdio`, which connects to (or starts) the HTTP server
- **Obsidian vault**: memories are markdown files with YAML frontmatter — browse them in Obsidian
- **Semantic search**: queries are embedded and matched against stored memories using cosine similarity

The stdio bridge answers the MCP handshake itself, so Claude Code connects instantly even while the server is starting or broken. Tool calls then report the server's startup error instead of timing out. The server listens on loopback only and requires a per-start token that only your user can read. It exits about a minute after the last session closes. The next session starts it again.

## MCP tools

| Tool | Description |
|------|-------------|
| `memory_store` | Store a new memory (decision, procedure, insight, concept) |
| `memory_search` | Semantic search across all memories |
| `memory_list` | Browse memories by category, tags, project, or date |
| `memory_update` | Update an existing memory |
| `memory_delete` | Delete a memory |
| `memory_reindex` | Rebuild the search index from vault files |

## Install

### Prerequisites

- Rust toolchain (edition 2024, Rust 1.88+)
- macOS or Linux (Windows is not supported)
- Network access on the first build: the ONNX Runtime binaries are downloaded by the `ort` crate
- For OpenAI embeddings: an API key

### Build and install

```bash
git clone https://github.com/dck/brain-mcp.git
cd brain-mcp
make install        # same as: cargo install --path brain-cli
```

Local ONNX embeddings are built in by default. For an OpenAI-only binary without ONNX Runtime:
`cargo install --path brain-cli --no-default-features`.

### First-time setup

```bash
brain-mcp init
```

Interactive wizard that configures:
- Vault path (where memories are stored as markdown files)
- Embedding provider (OpenAI API or local ONNX — no network needed)
- HTTP port and grace period

If you choose local ONNX, the model (~90MB) is downloaded automatically.

### Register with Claude Code

```bash
claude mcp add --scope user --transport stdio brain-mcp -- brain-mcp serve --stdio
```

To use a non-default config, register `brain-mcp --config /path/to/config.toml serve --stdio`. Only one server runs per user: `--config` applies when the proxy has to start the server, and it does not switch a server that is already running.

Restart Claude Code. The server starts automatically when Claude Code connects.

### Automatic recall (recommended)

Models rarely call `memory_search` unprompted — without a nudge, memory degrades
into write-only storage. Add a `SessionStart` hook so every session begins with a
compact index of your memories injected into context; the model then drills down
with `memory_search` when something looks relevant. In `~/.claude/settings.json`:

```json
{
  "hooks": {
    "SessionStart": [{
      "matcher": "startup|resume|clear",
      "hooks": [{
        "type": "command",
        "command": "brain-mcp recall --project \"$(basename \"$PWD\")\"",
        "timeout": 10
      }]
    }]
  }
}
```

`brain-mcp recall` reads the vault directly (no server, no embeddings), prints one
line per memory (newest first), and includes cross-project memories alongside the
`--project` ones.

## Architecture

Hexagonal (ports & adapters). The core domain has no knowledge of transport, storage backend, or embedding provider.

```
brain-cli (binary, wires everything)
  -> brain-server (axum HTTP, bearer auth, singleton, session leases)
       -> brain-mcp-proto (JSON-RPC 2.0, tool schemas, handler routing)
            -> brain-core (domain: MemoryService, port traits, models, config)
  -> brain-vault (VaultPort: markdown files + YAML frontmatter)
  -> brain-embed (EmbeddingPort: local ONNX (default feature) or OpenAI API)
  -> brain-index (IndexPort: rusqlite + cosine similarity)
```

## Vault structure

```
~/brain/
├── procedures/          # how-to guides, deploy steps
├── decisions/           # architectural choices, trade-offs
├── learnings/           # debugging insights, TILs
├── concepts/            # reference knowledge, patterns
├── projects/            # project-specific context
└── _templates/          # category templates (optional)
```

Each memory is a markdown file with YAML frontmatter:

```yaml
---
title: "Fix flaky tests caused by state leakage"
tags: [testing, debugging, ci]
created_at: "2026-03-28T14:30:00Z"
project: maestro
category: learnings
id: "20260328-fix-flaky-tests-caused-by-state-leakage"
---

The integration tests in the auth module were flaking because...
```

## CLI

```bash
brain-mcp init       # interactive setup wizard
brain-mcp serve      # start HTTP server (foreground)
brain-mcp recall     # print memory index (for SessionStart hooks)
brain-mcp status     # show server status
brain-mcp stop       # stop the server
brain-mcp reindex    # rebuild search index from vault
```

## Configuration

Default location: `~/.config/brain-mcp/config.toml` (Linux) or `~/Library/Application Support/brain-mcp/config.toml` (macOS).

```toml
[vault]
path = "~/brain"
categories = ["procedures", "decisions", "learnings", "concepts", "projects"]

[embedding]
provider = "onnx"                    # or "openai"
model = "all-MiniLM-L6-v2"
model_path = "~/.config/brain-mcp/models/all-MiniLM-L6-v2"

[index]
path = "~/.config/brain-mcp/index.db"

[server]
http_port = 47200
grace_period_seconds = 60
```

Older configs may contain `backend = "sqlite-vec"`; it is still accepted and means the built-in SQLite index.

## Troubleshooting

- `brain-mcp status` shows the server PID, version, live sessions and the log path.
- A server started by Claude Code logs to `server.log` in the state directory
  (`~/Library/Application Support/brain-mcp/run/` on macOS, `~/.config/brain-mcp/run/` on Linux).
  If a tool call reports "brain-mcp server is unavailable", the message quotes the end of that log.
- `brain-mcp stop` shuts the server down; the next tool call starts a fresh one.
- Set `grace_period_seconds = 0` under `[server]` to keep the server running when no session is open.

## License

MIT, see [LICENSE](LICENSE).
