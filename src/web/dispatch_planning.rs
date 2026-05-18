//! Dispatch handlers for planning and verification tools.

use crate::web::state::AppState;

/// Dispatch verify_code tool — runs the verification pipeline.
pub async fn dispatch_verify_code(args: &serde_json::Value) -> anyhow::Result<String> {
    let run_tests = args["run_tests"].as_bool().unwrap_or(true);
    let browser_url = args["browser_url"].as_str().map(String::from);

    let config = crate::verification::pipeline::VerificationConfig {
        run_tests,
        browser_url,
        ..Default::default()
    };

    let result = crate::verification::pipeline::run_verification(&config).await?;

    if result.overall_pass {
        Ok(format!(
            "[VERIFICATION PASSED]\nBuild: ✅\nTests: {}\nBrowser: {}",
            if config.run_tests { "✅" } else { "skipped" },
            if result.browser_result.is_some() { "✅" } else { "skipped" },
        ))
    } else {
        Ok(crate::verification::pipeline::format_fix_prompt(&result))
    }
}

/// Dispatch plan_and_execute tool — decomposes and executes a task DAG.
pub async fn dispatch_plan_and_execute(
    state: &AppState,
    args: &serde_json::Value,
) -> anyhow::Result<String> {
    use std::sync::atomic::{AtomicBool, Ordering};
    static DAG_RUNNING: AtomicBool = AtomicBool::new(false);

    // Prevent recursive DAG execution
    if DAG_RUNNING.swap(true, Ordering::SeqCst) {
        anyhow::bail!("plan_and_execute cannot be called recursively. Use individual tools instead.");
    }

    let result = dispatch_plan_inner(state, args).await;
    DAG_RUNNING.store(false, Ordering::SeqCst);
    result
}

async fn dispatch_plan_inner(
    state: &AppState,
    args: &serde_json::Value,
) -> anyhow::Result<String> {
    let objective = args["objective"].as_str().unwrap_or("");
    let context = args["project_context"].as_str().unwrap_or("");

    if objective.is_empty() {
        anyhow::bail!("Missing 'objective' parameter");
    }

    // Derive the full tool list from the authoritative registry.
    // Sub-agents spawned by plan_and_execute have the same tool access as any admin inference.
    let tool_names = layer2_tool_names();

    let provider = state.provider.as_ref();
    let dag = crate::planning::planner::decompose_objective(
        provider, objective, context, &tool_names,
    ).await?;

    let task_count = dag.nodes.len();
    let mut dag = dag;
    let result = crate::planning::executor::execute_dag(
        provider, state, &mut dag, &tool_names,
    ).await?;

    Ok(format!(
        "[DAG Execution Complete]\nObjective: {}\n\
         Tasks: {} total, {} completed, {} failed, {} blocked\n\
         Success: {}\n\n{}",
        objective, task_count, result.completed, result.failed, result.blocked,
        result.overall_success, result.summary
    ))
}


/// Extract tool names from `layer2_tools()` — the authoritative registry for admin tool access.
/// Called at request time so the list always reflects the live schema.
fn layer2_tool_names() -> Vec<String> {
    let schema = crate::tools::schema::layer2_tools();
    schema.as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|t| t["function"]["name"].as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_layer2_tool_names_includes_project() {
        let names = layer2_tool_names();
        assert!(names.contains(&"project".to_string()), "layer2 must include project");
        assert!(names.contains(&"create_artifact".to_string()), "layer2 must include create_artifact");
        assert!(names.contains(&"memory".to_string()), "layer2 must include memory");
    }

    #[test]
    fn test_layer2_tool_names_non_empty() {
        let names = layer2_tool_names();
        assert!(!names.is_empty());
    }
}
