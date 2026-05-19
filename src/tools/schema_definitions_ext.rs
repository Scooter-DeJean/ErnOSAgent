// Ern-OS — Extended tool schema definitions (session recall + introspection).
// Split from schema_definitions.rs for governance compliance (<500 lines).

pub fn session_recall_tool_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": "session_recall",
            "description": "Browse and read your prior chat sessions for additional context. Actions: 'list' (paginated index), 'get' (full session), 'summary' (topic digest), 'search' (query match), 'topics' (subject list)",
            "parameters": {
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["list", "get", "summary", "search", "topics"], "description": "Session operation" },
                    "session_id": { "type": "string", "description": "Session ID (for get/summary/topics)" },
                    "query": { "type": "string", "description": "Search query (for search)" },
                    "page": { "type": "integer", "description": "Page number (for list, default 1)" },
                    "per_page": { "type": "integer", "description": "Results per page (for list, default 10)" },
                    "limit": { "type": "integer", "description": "Max results (for search, default 10)" }
                },
                "required": ["action"]
            }
        }
    })
}

pub fn introspect_tool_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": "introspect",
            "description": "Access your own reasoning logs, agent activity, scheduler status, observer audits, and system health. Your self-awareness layer. Actions: 'reasoning_log', 'agent_activity', 'scheduler_status', 'observer_audit', 'system_status', 'digest_status', 'my_tools'",
            "parameters": {
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["reasoning_log", "agent_activity", "scheduler_status", "observer_audit", "system_status", "digest_status", "my_tools"], "description": "Introspection operation" },
                    "limit": { "type": "integer", "description": "Max entries (for reasoning_log/agent_activity/observer_audit)" },
                    "session_id": { "type": "string", "description": "Session ID (for reasoning_log — defaults to current/most recent)" }
                },
                "required": ["action"]
            }
        }
    })
}

pub fn project_tool_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": "project",
            "description": "Manage long-form writing projects (novels, journalism, research). Actions: 'create' (new project), 'list' (all projects), 'status' (project stats), 'bible' (manage Story Bible — characters, world, timeline, themes, style notes). Use bible_action for Story Bible operations: add, list, search, remove.",
            "parameters": {
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["create", "list", "status", "bible"], "description": "Project operation" },
                    "name": { "type": "string", "description": "Project name (for create)" },
                    "project_id": { "type": "string", "description": "Project ID (for status/bible)" },
                    "bible_action": { "type": "string", "enum": ["add", "list", "search", "remove"], "description": "Story Bible operation (when action=bible)" },
                    "category": { "type": "string", "enum": ["character", "world", "timeline", "theme", "style"], "description": "Bible entry category" },
                    "key": { "type": "string", "description": "Entry name (e.g. character name, location)" },
                    "value": { "type": "string", "description": "Entry content (e.g. character description)" },
                    "query": { "type": "string", "description": "Search query (for bible search)" }
                },
                "required": ["action"]
            }
        }
    })
}

pub fn audiobook_tool_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": "audiobook",
            "description": "Generate audiobooks from manuscripts via the script-reader engine (localhost:8000). Actions: 'parse' (detect characters + chapters), 'voices' (list available voices), 'assign' (override voice assignments), 'generate' (start audiobook generation), 'status' (check progress).",
            "parameters": {
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["parse", "voices", "assign", "generate", "status"], "description": "Audiobook operation" },
                    "script": { "type": "string", "description": "Manuscript text content (for parse)" },
                    "assignments": { "type": "object", "description": "Voice assignments map (for assign)" },
                    "project_name": { "type": "string", "description": "Output project name (for generate)" },
                    "url": { "type": "string", "description": "script-reader URL (default: http://localhost:8000)" }
                },
                "required": ["action"]
            }
        }
    })
}
