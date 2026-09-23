# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Test Commands

```bash
cargo build --workspace            # build all crates
cargo build --release              # release build (binary at target/release/brain-mcp)
cargo test --workspace             # run all tests
cargo test -p brain-core           # test a single crate
cargo test -p brain-core service   # test a specific module
cargo test -- test_name            # run a specific test by name
cargo clippy --workspace --all-targets -- -D warnings   # lint (CI runs exactly this)
cargo fmt --all                    # format
```

## Architecture

Hexagonal (ports & adapters). The dependency graph is strictly layered:

```
brain-cli (binary, wires everything)
  -> brain-server (axum HTTP, bearer auth, singleton, session leases)
       -> brain-mcp-proto (JSON-RPC 2.0, tool schemas, McpHandler routing)
            -> brain-core (domain: MemoryService, port traits, models, config)
  -> brain-vault (VaultPort impl: markdown files + YAML frontmatter)
  -> brain-embed (EmbeddingPort impl: local ONNX (default feature) or OpenAI API)
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

Initialize result, protocol-version negotiation, instructions text and the `tool_text`/`tool_error` helpers live in `brain-mcp-proto/src/mcp.rs`. The server handler and the stdio proxy both use them. Tool failures are `isError` results. Missing arguments and unknown tools are JSON-RPC `-32602`.

### Singleton server and stdio proxy

One `brain-mcp serve` process per user owns the embedding model, SQLite index and vault writes. MCP clients run `brain-mcp serve --stdio` (`brain-cli/src/commands/proxy.rs`), a thin proxy that:

- answers `initialize`, `ping`, `tools/list` and notifications locally, so connecting never waits on the server;
- starts or finds the server in the background, and forwards only `tools/call` with the bearer token from the state file;
- restarts a server whose binary was reinstalled after it started or whose version is older, and kills one that holds the lock but does not answer `/health`;
- holds a session lease (heartbeat every 15s, 45s expiry) and closes it on exit.

The server (`brain-server`) takes an `fs2` lock on `<state_dir>/brain-mcp.state`, binds 127.0.0.1 (falling back to a random port if `http_port` is taken), then writes the state file (mode 0600: pid, url, version, token). Routes: `GET /health` (public identity), `POST /mcp`, `POST /shutdown`, `POST /session/heartbeat`, `POST /session/close` (bearer token; Host must be loopback; any Origin is rejected). It exits on SIGINT/SIGTERM, on `/shutdown`, or `grace_period_seconds` after the last session lease ends; the lock and state file are released on exit. A server spawned by the proxy logs to `<state_dir>/server.log`.

`<state_dir>` is `<config_dir>/run`, where `<config_dir>` is `~/Library/Application Support/brain-mcp` on macOS and `~/.config/brain-mcp` on Linux. Unix-only (flock, setsid, signals): macOS and Linux are supported, Windows is not.

## Configuration

Default: `<config_dir>/config.toml` (see above; created by `brain-mcp init`). `--config <path>` overrides it and is passed on to a server the proxy spawns. Reference config at `config/default.toml`. Config structs in `brain-core/src/config.rs`. Paths with `~` are expanded via `Config::resolve_paths()`.

## Testing patterns

- All crates use inline `#[cfg(test)]` modules with `#[tokio::test]`
- Mock ports in `brain-core/src/mocks.rs` (MockVault, MockEmbedder, MockIndex) are `pub` for use across crates
- `brain-vault` tests use `tempfile::tempdir()` for filesystem isolation
- `brain-embed` tests use `wiremock` to mock the OpenAI API
- `brain-index` tests use `SqliteVecIndex::open_in_memory()`
- `brain-cli/tests/e2e.rs` — in-process HTTP server on a random port, exercises all tools via reqwest with a bearer token
- `brain-cli/tests/stdio_proxy.rs`, `brain-cli/tests/lifecycle.rs` — run the real binary with `HOME` set to a tempdir (never the real config/state)

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
