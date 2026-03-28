use serde_json::{json, Value};

pub fn tool_definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "memory_store",
            "description": "Store a new memory",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "content": { "type": "string", "description": "The memory content" },
                    "title": { "type": "string", "description": "A short title for the memory" },
                    "tags": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Tags for categorizing the memory"
                    },
                    "category": {
                        "type": "string",
                        "description": "Category (default: learnings)",
                        "default": "learnings"
                    },
                    "project": {
                        "type": "string",
                        "description": "Optional project name"
                    }
                },
                "required": ["content", "title", "tags"]
            }
        }),
        json!({
            "name": "memory_search",
            "description": "Search memories by semantic similarity",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search query" },
                    "limit": {
                        "type": "integer",
                        "description": "Max results to return (default: 5)",
                        "default": 5
                    },
                    "tags": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Filter by tags"
                    }
                },
                "required": ["query"]
            }
        }),
        json!({
            "name": "memory_list",
            "description": "List memories with optional filters",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "tags": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Filter by tags"
                    },
                    "since": {
                        "type": "string",
                        "description": "Only include memories created after this ISO8601 timestamp"
                    },
                    "category": { "type": "string", "description": "Filter by category" },
                    "project": { "type": "string", "description": "Filter by project" }
                },
                "required": []
            }
        }),
        json!({
            "name": "memory_update",
            "description": "Update an existing memory",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "The memory ID" },
                    "content": { "type": "string", "description": "New content" },
                    "title": { "type": "string", "description": "New title" },
                    "tags": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "New tags"
                    }
                },
                "required": ["id"]
            }
        }),
        json!({
            "name": "memory_delete",
            "description": "Delete a memory by ID",
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
            "description": "Rebuild the search index from all stored memories",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "required": []
            }
        }),
    ]
}
