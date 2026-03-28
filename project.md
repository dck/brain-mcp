# brain-mcp — Project Specification v1

> MCP server that gives AI coding agents persistent, cross-project memory backed by an Obsidian-compatible markdown vault with semantic vector search.

## Problem

AI coding agents (Claude Code, Copilot CLI, etc.) lose all context between sessions. Each new session starts from zero. Users working across many projects repeatedly re-explain procedures, decisions, and debugging insights that the agent already helped with days or weeks ago.

## Solution

A singleton MCP server that any agent can connect to. It stores memories as structured markdown files in an Obsidian vault and indexes them for semantic search. The agent calls `memory_store` and `memory_search` — it never knows about files, embeddings, or SQLite underneath.

This is a **personal tool** — single-user, local-only, not scoped to any project or team. It runs as a user-level service and is configured once per machine.

---

## Architecture

Hexagonal (ports & adapters). The core domain has no knowledge of transport, storage backend, or embedding provider.

```
┌─────────────────────────────────────────────────────┐
│                    Transports (in)                   │
│              HTTP/SSE  │  Unix socket                │
└────────────────┬───────┴──────┬──────────────────────┘
                 │              │
    ┌────────────▼──────────────▼──────────────┐
    │              MCP Protocol Layer           │
    │         (JSON-RPC 2.0 + tool router)      │
    └────────────────────┬─────────────────────┘
                         │
    ┌────────────────────▼─────────────────────┐
    │              Core Domain                  │
    │                                           │
    │  MemoryService                            │
    │    - store(content, tags, title, project)  │
    │    - search(query, limit, tags?)          │
    │    - list(tags?, since?, category?)       │
    │    - update(id, content?, tags?)          │
    │    - delete(id)                           │
    │    - reindex()                            │
    │                                           │
    │  Ports (traits):                          │
    │    - VaultPort      (read/write markdown)  │
    │    - EmbeddingPort  (text → vector)        │
    │    - IndexPort      (store/query vectors)  │
    └───┬──────────────┬──────────────┬─────────┘
        │              │              │
   ┌────▼────┐   ┌────▼────┐   ┌────▼─────┐
   │  Vault   │   │Embedding│   │  Index    │
   │ Adapter  │   │ Adapter │   │ Adapter   │
   │          │   │         │   │           │
   │ Obsidian │   │ OpenAI  │   │sqlite-vec │
   │ markdown │   │ Voyage  │   │ libSQL    │
   │ files    │   │ ONNX    │   │ (future)  │
   └──────────┘   └─────────┘   └───────────┘
```

---

## Language & Toolchain

- **Language**: Rust (edition 2021)
- **Async runtime**: tokio
- **MCP protocol**: hand-rolled JSON-RPC 2.0 (minimal, no heavy framework)
- **SQLite**: rusqlite + sqlite-vec extension (default index adapter)
- **ONNX**: ort crate (optional, behind feature flag `local-embeddings`)
- **HTTP**: axum
- **Config**: toml + clap for CLI
- **Filesystem watch**: notify crate (wraps kqueue/inotify)

---

## MCP Tools Exposed

The agent sees these tools. It does not know about files, vectors, or databases.

### Core tools

| Tool | Parameters | Description |
|------|-----------|-------------|
| `memory_store` | `content`, `title`, `tags[]`, `project?`, `category?` | Store a new memory |
| `memory_search` | `query`, `limit?` (default 5), `tags?[]` | Semantic vector search |
| `memory_list` | `tags?[]`, `since?`, `category?`, `project?` | Browse by filter (no embeddings) |
| `memory_update` | `id`, `content?`, `tags?[]`, `title?` | Update an existing memory |
| `memory_delete` | `id` | Delete a memory (removes file + vector) |
| `memory_reindex` | — | Full reindex: nuke index, re-walk vault, re-embed |

### v2 tools

| Tool | Description |
|------|-------------|
| `memory_related` | Find memories related to a given memory (by vector similarity) |
| `memory_stats` | Vault stats: count by category, tag cloud, recent activity |
| `memory_link` | Explicitly create a `[[wikilink]]` between two memories |

---

## Vault Structure (Obsidian-compatible)

Category-based with tags for cross-cutting concerns.

```
~/brain/                          # vault root (configurable)
├── procedures/                   # how-to guides, deploy steps, workflows
│   ├── deploy-new-app.md
│   └── setup-dev-environment.md
├── decisions/                    # architectural choices, trade-offs, ADRs
│   ├── chose-libsql-over-sqlite-vec.md
│   └── grpc-vs-rest-for-maestro.md
├── learnings/                    # debugging insights, mistakes, TILs
│   ├── flaky-tests-state-leakage.md
│   └── git-rebase-onto-after-squash.md
├── concepts/                     # reference knowledge, patterns, explanations
│   ├── stacked-prs-workflow.md
│   └── mcp-protocol-basics.md
├── projects/                     # project-specific context (sub-folders)
│   ├── maestro/
│   │   └── insights-mvp-architecture.md
│   └── distill/
│       └── chunking-algorithm-comparison.md
├── _templates/                   # templates for each category (not indexed)
│   ├── procedure.md
│   ├── decision.md
│   ├── learning.md
│   └── concept.md
└── _index.md                     # auto-generated vault overview (optional)
```

### Frontmatter schema

Every memory file has YAML frontmatter:

```yaml
---
title: "Deploy new application via terraform/helm/ansible"
tags:
  - deploy
  - terraform
  - helm
  - ansible
created_at: 2026-03-28T14:30:00Z
project: maestro
category: procedures
id: 20260328-143000-deploy-new-app
---
```

- `id`: generated by the server — `YYYYMMDD-HHMMSS-slugified-title`
- `category`: determines the folder
- `tags`: free-form, used for filtering and Obsidian tag navigation
- `project`: optional — omitted for cross-project knowledge
- `created_at`: ISO 8601

### Wikilinks

The server supports `[[wikilinks]]` in memory content. When an agent stores a memory that references another, it can include `[[deploy-new-app]]` and Obsidian's graph view will show the connection. The `memory_link` tool (v2) can add links programmatically.

### Templates

Each category has a template in `_templates/`. When the agent calls `memory_store` with `category: "procedures"`, the server reads `_templates/procedure.md`, fills in frontmatter, appends the content. Templates are user-editable markdown files.

Example `_templates/procedure.md`:

```markdown
---
title: "{{title}}"
tags: {{tags}}
created_at: {{created_at}}
project: {{project}}
category: procedures
id: {{id}}
---

## Overview

{{content}}

## Steps

## Notes
```

---

## Storage Adapters

### Vault Adapter

- Reads/writes markdown files to the vault directory
- Watches filesystem for external changes (notify crate: kqueue on macOS, inotify on Linux)
- On external change: re-embeds changed file, updates index
- On `memory_store`: writes file, embeds, indexes — atomic operation

### Embedding Adapter (configurable via `[embedding]` config)

**API mode** (default):
- Calls OpenAI `text-embedding-3-small` (or configurable provider/model)
- ~150ms latency per embed (network)
- Zero local RAM for model
- Requires API key via env var

**Local ONNX mode** (feature flag `local-embeddings`):
- Loads `bge-micro-v2` via `ort` crate
- ~5ms latency per embed
- ~100MB additional RAM
- No network needed

Both implement:

```rust
#[async_trait]
trait EmbeddingPort: Send + Sync {
    async fn embed(&self, text: &str) -> Result<Vec<f32>>;
    fn dimensions(&self) -> usize;
}
```

### Index Adapter (configurable via `[index]` config)

**sqlite-vec** (default):
- SQLite extension loaded via rusqlite
- Brute-force KNN, SIMD-accelerated
- FTS5 for keyword search alongside vector search
- Single `.db` file, disposable cache

**libSQL** (alternative adapter, future):
- Vectors as native column type
- Full SQL WHERE on vector queries
- DiskANN optional indexing

Both implement:

```rust
#[async_trait]
trait IndexPort: Send + Sync {
    async fn upsert(&self, id: &str, embedding: &[f32], metadata: &Metadata) -> Result<()>;
    async fn search(&self, embedding: &[f32], limit: usize, filter: &Filter) -> Result<Vec<SearchResult>>;
    async fn delete(&self, id: &str) -> Result<()>;
    async fn list(&self, filter: &Filter) -> Result<Vec<Metadata>>;
    async fn clear(&self) -> Result<()>;
}
```

The index is a **cache**. It can be deleted and rebuilt from the vault at any time via `memory_reindex`.

---

## Singleton Lifecycle

### Runtime files

```
~/.config/brain-mcp/run/
├── brain-mcp.pid          # PID number
└── brain-mcp.conn         # connection details (TOML)
```

`brain-mcp.conn`:

```toml
pid = 48291
http = "http://localhost:47200/mcp"
unix = "/tmp/brain-mcp.sock"
started_at = "2026-03-28T14:30:00Z"
```

### Startup

1. Check PID file. If exists:
   a. `kill(pid, 0)` — if process alive AND process name matches `brain-mcp` → print connection info from `.conn` file, exit.
   b. If process dead or name mismatch → stale PID file, remove it.
2. Start server, write PID file and `.conn` file.
3. Begin listening on configured transports.

### Shutdown — reference counting

- Server tracks connected MCP clients (active transport sessions).
- When a client disconnects, decrement count.
- When count reaches 0, start a grace period (configurable, default 60s).
- If no new client connects within grace period, server exits and removes PID + conn files.
- On SIGTERM/SIGINT: immediate graceful shutdown regardless of client count.

---

## Transport Adapters

All transports speak the same MCP JSON-RPC 2.0 protocol. Each is a separate adapter behind a `TransportPort`.

### HTTP/SSE (primary)

- Default: `http://localhost:47200/mcp` (port configurable)
- SSE for server-to-client notifications (e.g., reindex progress)

### Unix socket

- Default: `$XDG_RUNTIME_DIR/brain-mcp.sock` or `/tmp/brain-mcp.sock`
- No port allocation, local only

**Note:** stdio transport is intentionally omitted from v1. All modern MCP clients support HTTP. If a stdio adapter is needed later, it would be a thin shim binary that bridges stdio to the running HTTP/Unix socket server — not a separate server instance.

---

## CLI

```bash
brain-mcp init                     # interactive first-time setup
brain-mcp serve                    # start server (foreground)
brain-mcp serve --daemonize        # start in background
brain-mcp status                   # show running/stopped, connection info, client count
brain-mcp reindex                  # trigger reindex (connects to running server, or starts one)
brain-mcp stop                     # send shutdown signal
```

### `brain-mcp init`

Interactive first-time setup with sensible defaults:

```
$ brain-mcp init

Welcome to brain-mcp setup!

Vault path [~/brain]: ~/obsidian/brain
Categories [procedures, decisions, learnings, concepts, projects]: <enter to accept>

Embedding provider:
  1. OpenAI (recommended, requires API key)
  2. Voyage AI
  3. Local ONNX (no network, larger binary)
Choose [1]: 1

OpenAI API key env var [OPENAI_API_KEY]: <enter to accept>
Embedding model [text-embedding-3-small]: <enter to accept>

Index backend:
  1. sqlite-vec (default, simplest)
  2. libSQL
Choose [1]: 1

HTTP port [47200]: <enter to accept>
Enable Unix socket? [Y/n]: Y
Grace period after last client disconnects (seconds) [60]: <enter to accept>

✓ Created config at ~/.config/brain-mcp/config.toml
✓ Created vault structure at ~/obsidian/brain/
  ├── procedures/
  ├── decisions/
  ├── learnings/
  ├── concepts/
  ├── projects/
  └── _templates/ (5 templates)
✓ Created index database at ~/.config/brain-mcp/index.db

To connect Claude Code, add this to your global MCP config:

  claude mcp add --scope user --transport http brain-mcp http://localhost:47200/mcp

Add this to ~/.claude/CLAUDE.md:

  ## Memory
  You have persistent cross-project memory via the brain-mcp MCP server.
  On session start: search for memories related to the current task.
  Before session end: store key decisions, procedures, and debugging insights.
  Use tags for topics and project names.

Ready! Start with: brain-mcp serve
```

---

## Configuration

Location: `~/.config/brain-mcp/config.toml` (XDG), overridable with `--config`

```toml
[vault]
path = "~/brain"
templates_dir = "_templates"
categories = ["procedures", "decisions", "learnings", "concepts", "projects"]

[embedding]
provider = "openai"                       # "openai" | "voyage" | "onnx"
model = "text-embedding-3-small"
api_key_env = "OPENAI_API_KEY"            # env var name (never store key in config)
# For ONNX:
# provider = "onnx"
# model_path = "~/.config/brain-mcp/models/bge-micro-v2.onnx"

[index]
backend = "sqlite-vec"                    # "sqlite-vec" | "libsql"
path = "~/.config/brain-mcp/index.db"

[server]
transport = ["http", "unix"]
http_port = 47200
unix_socket = "/tmp/brain-mcp.sock"
grace_period_seconds = 60

[watch]
enabled = true
debounce_ms = 500
```

---

## Reindex Flow

### Full reindex (`memory_reindex` tool or `brain-mcp reindex` CLI)

1. Drop all rows from index database (or delete the `.db` file)
2. Walk vault directory, skip `_templates/` and dotfiles
3. For each `.md` file:
   a. Parse YAML frontmatter (title, tags, project, category, id)
   b. Extract body content
   c. Call embedding adapter → get vector
   d. Insert into index (id, vector, metadata)
4. Return count of indexed files

### Incremental update (filesystem watcher)

1. File changed → debounce (500ms default) → read file, parse frontmatter
2. If file has valid frontmatter with `id`:
   a. Re-embed content
   b. Upsert into index
3. If file deleted → delete from index by id
4. If new file without `id` in frontmatter → ignore (user-created note, not a memory)

---

## Agent Integration

brain-mcp is agent-agnostic. It exposes MCP tools, any MCP-compatible agent can use them. No hooks, no slash commands, no agent-specific code.

The user is responsible for:

1. **One-time MCP registration** — e.g., `claude mcp add --scope user --transport http brain-mcp http://localhost:47200/mcp` (printed by `brain-mcp init`)

2. **Agent instructions** — a snippet in the agent's global config (e.g., `~/.claude/CLAUDE.md`) telling it to use memory tools. Suggested snippet:

```markdown
## Memory
You have persistent cross-project memory via the brain-mcp MCP server.
- On session start: call memory_search with keywords related to the current task.
- Before session end: call memory_store to save key decisions, procedures, and debugging insights.
- Use descriptive tags for topics (e.g., deploy, terraform, debugging) and project names.
- Use category to organize: procedures, decisions, learnings, concepts, projects.
- Use [[wikilinks]] to reference related memories by their slugified title.
```

---

## Non-goals for v1

- No web UI / viewer (browse in Obsidian instead)
- No AI-powered compression (agent writes its own summaries)
- No cloud sync (use Obsidian Sync or git if needed)
- No multi-user / auth (single-user, local only)
- No chunking of large documents (one memory = one file = one vector)
- No stdio transport (all modern agents support HTTP)
- No agent-specific hooks, slash commands, or plugins
- No auto-installation into agent configs

---

## Future / v2 ideas

- `memory_related` tool using vector similarity between stored memories
- `memory_stats` tool with tag cloud, category counts, recent activity
- `memory_link` tool to programmatically add wikilinks
- Graph analysis (backlinks, hub detection) via petgraph
- Template validation (ensure frontmatter matches schema)
- Auto-categorization (suggest category based on content similarity)
- Export/import for migration between machines
- stdio shim binary for legacy agents
- Optional Obsidian plugin for bidirectional awareness
