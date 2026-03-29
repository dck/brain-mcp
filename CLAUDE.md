# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Test Commands

```bash
cargo build --workspace            # build all crates
cargo build --release              # release build (binary at target/release/brain-mcp)
cargo test --workspace             # run all 78 tests
cargo test -p brain-core           # test a single crate
cargo test -p brain-core service   # test a specific module
cargo test -- test_name            # run a specific test by name
cargo clippy --workspace           # lint (treat warnings as errors with -- -D warnings)
cargo fmt --all                    # format
```

## Architecture

Hexagonal (ports & adapters). The dependency graph is strictly layered:

```
brain-cli (binary, wires everything)
  -> brain-server (axum HTTP, singleton lifecycle, client tracking)
       -> brain-mcp-proto (JSON-RPC 2.0, tool schemas, McpHandler routing)
            -> brain-core (domain: MemoryService, port traits, models, config)
  -> brain-vault (VaultPort impl: markdown files + YAML frontmatter)
  -> brain-embed (EmbeddingPort impl: OpenAI API via reqwest)
  -> brain-index (IndexPort impl: rusqlite + BLOB vectors + cosine similarity)
```

`brain-core` has zero dependencies on any adapter crate. This is enforced at compile time.

### Port traits (`brain-core/src/ports.rs`)

Three async traits using `BoxFuture` (pin-boxed futures for dyn-compatibility):

- **VaultPort** — write/read/delete/list_all markdown files
- **EmbeddingPort** — embed text to vector, report dimensions and model_id
- **IndexPort** — upsert/search/delete/list/clear vectors, track stored model_id

Each has a real adapter crate and a mock in `brain-core/src/mocks.rs` for testing.

### MemoryService (`brain-core/src/service.rs`)

The orchestrator. Takes `Arc<dyn VaultPort>`, `Arc<dyn EmbeddingPort>`, `Arc<dyn IndexPort>`. Methods: `store`, `search`, `list`, `update`, `delete`, `reindex`, `check_model_compatibility`.

Key flow: `store()` generates ID (`YYYYMMDD-slug`), writes vault, embeds content, indexes. `search()` embeds query, searches index, hydrates full content from vault.

### MCP Protocol (`brain-mcp-proto/src/handler.rs`)

Hand-rolled JSON-RPC 2.0. McpHandler routes `initialize`, `tools/list`, `tools/call`. Six tools: `memory_store`, `memory_search`, `memory_list`, `memory_update`, `memory_delete`, `memory_reindex`. Tool responses wrap content in `{"content": [{"type": "text", "text": "..."}]}`.

### Singleton lifecycle (`brain-server/src/singleton.rs`)

File lock via `fs2` on `~/.config/brain-mcp/run/brain-mcp.state`. Lock acquired = you're the server. Lock held = already running (read state, exit). Lock released on drop, state file deleted. `ClientTracker` counts connections; grace period (default 60s) shuts down after last client disconnects.

## Configuration

Default: `~/.config/brain-mcp/config.toml` (created by `brain-mcp init`). Reference config at `config/default.toml`. Config structs in `brain-core/src/config.rs`. Paths with `~` are expanded via `Config::resolve_paths()`.

## Testing patterns

- All crates use inline `#[cfg(test)]` modules with `#[tokio::test]`
- Mock ports in `brain-core/src/mocks.rs` (MockVault, MockEmbedder, MockIndex) are `pub` for use across crates
- `brain-vault` tests use `tempfile::tempdir()` for filesystem isolation
- `brain-embed` tests use `wiremock` to mock the OpenAI API
- `brain-index` tests use `SqliteVecIndex::open_in_memory()`
- `brain-cli/tests/e2e.rs` — full roundtrip: starts HTTP server on random port, exercises all 6 tools via reqwest

## Vault file format

Memories are markdown files at `{vault_path}/{category}/{id}.md` with YAML frontmatter:

```yaml
---
title: "Deploy new app"
tags: [deploy, terraform]
created_at: "2026-03-28T14:30:00Z"
project: maestro          # omitted when None
category: procedures
id: "20260328-deploy-new-app"
---
Content body here.
```

Optional templates at `{vault_path}/_templates/{category}.md` use `{{placeholder}}` substitution.
