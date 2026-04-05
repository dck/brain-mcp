use serde_json::{Value, json};

pub fn tool_definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "memory_store",
            "description": "Store a memory only when it passes this test: (1) a future session would need this information, (2) it's not already in the codebase (code, README, CLAUDE.md), and (3) it took real effort to discover or decide. Good examples: deployment procedures, hard-won debugging insights, environment-specific setup steps, project conventions. Never store: summaries of completed work, refactoring plans, implementation details, generic knowledge. Write as if explaining to a future version of yourself with no context. Include the 'why', not just the 'what'.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "content": { "type": "string", "description": "The full memory content. Write clearly and include enough context to be useful without surrounding conversation." },
                    "title": { "type": "string", "description": "A concise, descriptive title (used in search results and as the filename slug)" },
                    "tags": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Tags for discovery and filtering. Use topic tags (deploy, terraform, auth), technology tags (rust, react, postgres), and project names."
                    },
                    "category": {
                        "type": "string",
                        "description": "One of: procedures (how-to guides, step-by-step), decisions (architectural choices, trade-offs), learnings (debugging insights, TILs, mistakes), concepts (reference knowledge, patterns). Default: learnings.",
                        "default": "learnings"
                    },
                    "project": {
                        "type": "string",
                        "description": "Project name if this memory is project-specific. Omit for cross-project knowledge."
                    }
                },
                "required": ["content", "title", "tags"]
            }
        }),
        json!({
            "name": "memory_search",
            "description": "Search persistent memories by semantic similarity. Use this at the start of a task to find relevant past context — previous decisions, known procedures, debugging lessons, or project-specific insights. Also use when the user references something that might have been stored previously, or when you need context about a topic, project, or technology the user has worked with before.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Natural language search query. Be specific — 'how to deploy maestro to production' works better than 'deploy'." },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of results (default: 5)",
                        "default": 5
                    },
                    "tags": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional tag filter. Only return memories that have ALL of these tags."
                    }
                },
                "required": ["query"]
            }
        }),
        json!({
            "name": "memory_list",
            "description": "Browse stored memories by filter criteria without semantic search. Use this to see what memories exist for a project, category, or tag — for example, listing all procedures, or all memories tagged with a specific technology.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "tags": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Only return memories with ALL of these tags"
                    },
                    "since": {
                        "type": "string",
                        "description": "Only include memories created after this ISO 8601 timestamp (e.g. 2026-03-01T00:00:00Z)"
                    },
                    "category": { "type": "string", "description": "Filter by category: procedures, decisions, learnings, or concepts" },
                    "project": { "type": "string", "description": "Filter by project name" }
                },
                "required": []
            }
        }),
        json!({
            "name": "memory_update",
            "description": "Update an existing memory. Use this to correct information, add new context to an existing memory, or update tags. Only the fields you provide will be changed; omitted fields keep their current values.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "The memory ID (format: YYYYMMDD-slugified-title)" },
                    "content": { "type": "string", "description": "New content (replaces existing content entirely)" },
                    "title": { "type": "string", "description": "New title" },
                    "tags": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "New tags (replaces existing tags entirely)"
                    }
                },
                "required": ["id"]
            }
        }),
        json!({
            "name": "memory_delete",
            "description": "Permanently delete a memory. Use when a memory is outdated, incorrect, or no longer relevant.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "The memory ID to delete" }
                },
                "required": ["id"]
            }
        }),
        json!({
            "name": "memory_reindex",
            "description": "Rebuild the entire search index from vault files. Use this after manually editing memory files in the vault, or if search results seem stale or incorrect. This is a heavy operation that re-embeds all memories.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "required": []
            }
        }),
    ]
}
