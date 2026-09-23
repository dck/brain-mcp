use serde_json::{Value, json};

pub const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];
pub const LATEST_PROTOCOL_VERSION: &str = "2025-06-18";
pub const SERVER_NAME: &str = "brain-mcp";
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn negotiate_protocol_version(params: Option<&Value>) -> &'static str {
    let requested = params
        .and_then(|p| p.get("protocolVersion"))
        .and_then(Value::as_str);
    SUPPORTED_PROTOCOL_VERSIONS
        .iter()
        .find(|v| Some(**v) == requested)
        .copied()
        .unwrap_or(LATEST_PROTOCOL_VERSION)
}

pub fn instructions() -> &'static str {
    "You have persistent cross-project memory via brain-mcp. \
        RECALL FIRST: call memory_search at the start of every session and every new task \
        (search the project name + task keywords) BEFORE acting. \
        Also search when the user says 'always', 'never', 'as usual', or 'we decided', \
        when something might have prior context (deployment, setup, a recurring issue), \
        and before proposing an approach the user may have already ruled out. \
        Searching is cheap and read-only — when in doubt, search. \
        Store sparingly: procedures that save time, hard-won debugging insights, \
        project conventions, environment-specific quirks. \
        Never store: work summaries, refactoring plans, implementation details, generic knowledge, \
        things already in code/README/CLAUDE.md. \
        Litmus test: would a future session need this AND is it not already in the codebase? \
        Write for your future self — include the why, not just the what. Use tags: project name + topic keywords."
}

pub fn initialize_result(params: Option<&Value>) -> Value {
    json!({
        "protocolVersion": negotiate_protocol_version(params),
        "capabilities": { "tools": {} },
        "serverInfo": { "name": SERVER_NAME, "version": SERVER_VERSION },
        "instructions": instructions(),
    })
}

pub fn tool_text(text: String) -> Value {
    json!({ "content": [{ "type": "text", "text": text }] })
}

pub fn tool_error(text: String) -> Value {
    json!({ "content": [{ "type": "text", "text": text }], "isError": true })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negotiate_echoes_supported_version() {
        let params = json!({"protocolVersion": "2025-03-26"});
        assert_eq!(negotiate_protocol_version(Some(&params)), "2025-03-26");
    }

    #[test]
    fn negotiate_falls_back_to_latest() {
        let params = json!({"protocolVersion": "1999-01-01"});
        assert_eq!(
            negotiate_protocol_version(Some(&params)),
            LATEST_PROTOCOL_VERSION
        );
        assert_eq!(negotiate_protocol_version(None), LATEST_PROTOCOL_VERSION);
    }

    #[test]
    fn initialize_result_shape() {
        let result = initialize_result(None);
        assert_eq!(result["serverInfo"]["name"], "brain-mcp");
        assert_eq!(result["serverInfo"]["version"], env!("CARGO_PKG_VERSION"));
        assert!(result["capabilities"]["tools"].is_object());
        assert!(
            result["instructions"]
                .as_str()
                .unwrap()
                .starts_with("You have persistent cross-project memory")
        );
    }

    #[test]
    fn tool_error_sets_is_error() {
        let value = tool_error("x".into());
        assert_eq!(value["isError"], true);
        assert_eq!(value["content"][0]["text"], "x");
    }
}
