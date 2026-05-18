//! Planning & Task DAG handlers.

use crate::web::state::AppState;
use axum::{extract::State, response::IntoResponse, Json};

/// Return current DAG status (if any active).
pub async fn dag_status(State(state): State<AppState>) -> impl IntoResponse {
    // Check if there's an active DAG in the session
    let data_dir = &state.config.general.data_dir;
    let dag_path = data_dir.join("active_dag.json");
    match tokio::fs::read_to_string(&dag_path).await {
        Ok(content) => {
            ([(axum::http::header::CONTENT_TYPE, "application/json")], content).into_response()
        }
        Err(_) => Json(serde_json::json!({
            "active": false,
            "message": "No active task plan"
        })).into_response(),
    }
}

/// Decompose an objective into a task DAG.
pub async fn decompose(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let objective = body["objective"].as_str().unwrap_or("").to_string();
    if objective.is_empty() {
        return (axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "objective is required"}))).into_response();
    }

    let provider = state.provider.as_ref();
    let project_context = format!("Ern-OS agent engine in {}", state.config.general.data_dir.display());

    let tool_schema = crate::tools::schema::layer2_tools();
    let tool_names: Vec<String> = tool_schema.as_array()
        .map(|arr| arr.iter().filter_map(|t| t["function"]["name"].as_str().map(String::from)).collect())
        .unwrap_or_default();

    match crate::planning::planner::decompose_objective(provider, &objective, &project_context, &tool_names).await {
        Ok(dag) => {
            // Persist active DAG
            let data_dir = &state.config.general.data_dir;
            let dag_json = serde_json::to_string_pretty(&dag).unwrap_or_default();
            let _ = tokio::fs::write(data_dir.join("active_dag.json"), &dag_json).await;
            Json(serde_json::json!({
                "active": true,
                "dag": serde_json::to_value(&dag).unwrap_or_default(),
            })).into_response()
        }
        Err(e) => (axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("{}", e)}))).into_response(),
    }
}
