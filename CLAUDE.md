# brain-mcp

Persistent cross-project memory MCP server for AI coding agents.
Obsidian-compatible markdown vault with semantic vector search.

## Build

```bash
cargo build --release
```

## Test

```bash
cargo test --workspace
```

## Architecture

Hexagonal (ports & adapters). Core domain has no knowledge of transport, storage, or embedding provider.

## Crate layout

- `brain-core` — domain models, port traits, MemoryService
- `brain-vault` — Obsidian markdown vault adapter (frontmatter, templates)
- `brain-embed` — embedding provider adapters (OpenAI)
- `brain-index` — sqlite-vec index adapter (rusqlite, BLOB vectors, cosine similarity)
- `brain-mcp-proto` — MCP JSON-RPC 2.0 protocol layer, tool schemas, handler
- `brain-server` — axum HTTP transport, flock singleton, client tracking
- `brain-cli` — CLI binary (`brain-mcp` command: init, serve, status, stop, reindex)

## First-time setup

```bash
brain-mcp init       # interactive wizard
brain-mcp serve      # start server
```

## MCP tools

memory_store, memory_search, memory_list, memory_update, memory_delete, memory_reindex
