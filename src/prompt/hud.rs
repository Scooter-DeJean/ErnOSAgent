// Ern-OS — High-performance, model-neutral Rust AI agent engine
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! Dynamic HUD — regenerated before every inference call.

/// Runtime context for the HUD.
pub struct HudContext {
    pub model_name: String,
    pub provider: String,
    pub context_length: usize,
    pub session_id: String,
    pub turn_count: usize,
    pub platform: String,
    pub timeline_count: usize,
    pub lesson_count: usize,
    pub procedure_count: usize,
    pub scratchpad_count: usize,
    pub document_count: usize,
    pub golden_count: usize,
    pub rejection_count: usize,
    pub curriculum_count: usize,
    pub review_total: usize,
    pub review_due: usize,
    pub quarantine_count: usize,
    pub observer_enabled: bool,
    pub conversation_stack: Option<crate::prompt::conversation_stack::ConversationStack>,
    // ── Phase 1-12 additions ──
    pub relevant_lessons: Vec<(f32, String)>,
    pub relevant_procedures: Vec<String>,
    pub context_usage_pct: f32,
    pub scratchpad_content: Option<String>,
    pub system_log_tail: String,
    pub kg_snapshot: String,
    pub reasoning_traces: Vec<String>,
    pub active_steering: Option<String>,
    pub platform_status: String,
    pub timeline_narrative: String,
    pub user_preferences: Option<String>,
    pub scheduler_status: String,
    /// Number of times context has been consolidated this session.
    /// Non-zero = model must use session_recall/memory before claiming data is absent.
    pub consolidation_count: usize,
}

/// Build the dynamic HUD string from live system state.
pub fn build_hud(ctx: &HudContext) -> String {
    let utc_now = chrono::Utc::now();
    let local_now = chrono::Local::now();

    let mut hud = format_hud_template(ctx, &utc_now, &local_now);

    hud.push_str(&format_lessons_section(&ctx.relevant_lessons));
    hud.push_str(&format_procedures_section(&ctx.relevant_procedures));
    hud.push_str(&format_scratchpad_section(&ctx.scratchpad_content));
    hud.push_str(&format_kg_section(&ctx.kg_snapshot));
    hud.push_str(&format_reasoning_section(&ctx.reasoning_traces));
    hud.push_str(&format_steering_section(&ctx.active_steering));
    hud.push_str(&format_log_section(&ctx.system_log_tail));
    hud.push_str(&format_timeline_section(&ctx.timeline_narrative));
    hud.push_str(&format_preferences_section(&ctx.user_preferences));
    hud.push_str(&format_consolidation_notice(ctx.consolidation_count));

    if let Some(ref stack) = ctx.conversation_stack {
        let section = stack.to_hud_section();
        if !section.is_empty() {
            hud.push_str(&section);
        }
    }

    hud
}

/// Core HUD template with numeric status lines.
fn format_hud_template(
    ctx: &HudContext,
    utc_now: &chrono::DateTime<chrono::Utc>,
    local_now: &chrono::DateTime<chrono::Local>,
) -> String {
    format!(
        "# System State (Live)\n\n\
         ## Ground Truth — Current Date & Time\n\
         This is live data from the host system clock. It is the authoritative source for all temporal reasoning.\n\
         - UTC:   {}\n\
         - Local: {}\n\n\
         ## Active Model\n\
         - Name: {}\n\
         - Provider: {}\n\
         - Context: {} tokens (usage: {:.0}%)\n\n\
         ## Session\n\
         - ID: {}\n\
         - Turn: {}\n\
         - Platform: {}\n\n\
         ## Memory\n\
         - Timeline: {} entries\n\
         - Lessons: {} rules\n\
         - Procedures: {} skills\n\
         - Scratchpad: {} notes\n\
         - Documents: {} chunks\n\n\
         ## Learning Buffers\n\
         - Golden (SFT): {} samples\n\
         - Rejection (DPO): {} pairs\n\
         - Quarantine: {} pending\n\n\
         ## Curriculum\n\
         - Courses: {}\n\
         - Review cards: {} total, {} due\n\n\
         ## Scheduler\n\
         {}\n\n\
         ## Platforms\n\
         {}\n\n\
         ## Observer: {}",
        utc_now.format("%A, %B %d, %Y at %H:%M:%S UTC"),
        local_now.format("%A, %B %d, %Y at %H:%M:%S %Z"),
        ctx.model_name, ctx.provider, ctx.context_length, ctx.context_usage_pct * 100.0,
        ctx.session_id, ctx.turn_count, ctx.platform,
        ctx.timeline_count, ctx.lesson_count, ctx.procedure_count, ctx.scratchpad_count, ctx.document_count,
        ctx.golden_count, ctx.rejection_count, ctx.quarantine_count,
        ctx.curriculum_count, ctx.review_total, ctx.review_due,
        ctx.scheduler_status,
        ctx.platform_status,
        if ctx.observer_enabled { "Enabled" } else { "Disabled" },
    )
}

/// Phase 1: Auto-matched lessons injected into context.
fn format_lessons_section(lessons: &[(f32, String)]) -> String {
    if lessons.is_empty() { return String::new(); }
    let mut out = String::from("\n\n## Active Lessons (Auto-Matched)\n");
    for (i, (conf, rule)) in lessons.iter().enumerate() {
        out.push_str(&format!("{}. [{:.1} conf] {}\n", i + 1, conf, rule));
    }
    out
}

/// Phase 2: Auto-matched procedures surfaced as system notice.
fn format_procedures_section(procs: &[String]) -> String {
    if procs.is_empty() { return String::new(); }
    format!(
        "\n\n## [SYSTEM NOTICE] Matching Procedures Detected\n\
         Procedures: {}\n\
         Use the `memory` tool with action `recall` to review these before proceeding.",
        procs.join(", ")
    )
}

/// Phase 4: Scratchpad content (not just count).
fn format_scratchpad_section(content: &Option<String>) -> String {
    match content {
        Some(c) if !c.is_empty() => format!("\n\n## Scratchpad (Active Notes)\n{}", c),
        _ => String::new(),
    }
}

/// Phase 6: Knowledge graph snapshot.
fn format_kg_section(snapshot: &str) -> String {
    if snapshot.is_empty() { return String::new(); }
    format!("\n\n## Knowledge Graph\n{}", snapshot)
}

/// Phase 7: Recent reasoning traces.
fn format_reasoning_section(traces: &[String]) -> String {
    if traces.is_empty() { return String::new(); }
    let mut out = String::from("\n\n## Recent Reasoning (Last 3)\n");
    for (i, trace) in traces.iter().enumerate() {
        out.push_str(&format!("{}. {}\n", i + 1, trace));
    }
    out
}

/// Phase 8: Active steering vectors.
fn format_steering_section(steering: &Option<String>) -> String {
    match steering {
        Some(s) if !s.is_empty() => format!("\n\n## Active Steering\n{}", s),
        _ => String::new(),
    }
}

/// Phase 5: System log tail (WARN/ERROR only).
fn format_log_section(tail: &str) -> String {
    if tail.is_empty() || tail == "No recent errors." { return String::new(); }
    format!("\n\n## Recent System Logs\n{}", tail)
}

/// Phase 10: Timeline narrative summary.
fn format_timeline_section(narrative: &str) -> String {
    if narrative.is_empty() { return String::new(); }
    format!("\n\n## Recent Timeline\n{}", narrative)
}

/// Phase 11: User preferences / Theory of Mind.
fn format_preferences_section(prefs: &Option<String>) -> String {
    match prefs {
        Some(p) if !p.is_empty() => format!("\n\n## User Preferences\n{}", p),
        _ => String::new(),
    }
}

/// PR8: Consolidation notice — alerts the model that history has been compressed.
/// When consolidation_count > 0, instructs the model to use memory tools before
/// claiming that information is absent from memory.
fn format_consolidation_notice(count: usize) -> String {
    if count == 0 { return String::new(); }
    format!(
        "\n\n## [CONTEXT COMPRESSED] — {} consolidation(s) this session\n\
         Earlier conversation history has been compressed and sorted into memory. \
         If a user asks about something that happened earlier or claims you should \
         know a fact, ALWAYS search memory first before concluding data is absent:\n\
         - `synaptic(action='search', concept='...')` — search the knowledge graph\n\
         - `scratchpad(action='list')` — check pinned facts\n\
         - `memory(action='recall', query='...')` — full memory recall\n\
         - `session_recall(action='list')` — browse compressed session history",
        count
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_ctx() -> HudContext {
        HudContext {
            model_name: "gemma-4-31B".to_string(),
            provider: "llamacpp".to_string(),
            context_length: 131072,
            session_id: "abc123".to_string(),
            turn_count: 5,
            platform: "web".to_string(),
            timeline_count: 42,
            lesson_count: 8,
            procedure_count: 3,
            scratchpad_count: 12,
            document_count: 5,
            golden_count: 2,
            rejection_count: 0,
            curriculum_count: 3,
            review_total: 15,
            review_due: 4,
            quarantine_count: 1,
            observer_enabled: true,
            conversation_stack: None,
            relevant_lessons: vec![],
            relevant_procedures: vec![],
            context_usage_pct: 0.256,
            scratchpad_content: None,
            system_log_tail: String::new(),
            kg_snapshot: String::new(),
            reasoning_traces: vec![],
            active_steering: None,
            platform_status: "Discord: connected".to_string(),
            timeline_narrative: String::new(),
            user_preferences: None,
            scheduler_status: "No jobs configured".to_string(),
            consolidation_count: 0,
        }
    }

    #[test]
    fn test_build_hud() {
        let ctx = make_ctx();
        let hud = build_hud(&ctx);
        assert!(hud.contains("gemma-4-31B"));
        assert!(hud.contains("131072"));
        assert!(hud.contains("abc123"));
        assert!(hud.contains("Turn: 5"));
        assert!(hud.contains("Platform: web"));
        assert!(hud.contains("Timeline: 42"));
        assert!(hud.contains("Enabled"));
        assert!(hud.contains("26%")); // context usage
        assert!(hud.contains("Discord: connected"));
    }

    #[test]
    fn test_hud_contains_time() {
        let ctx = make_ctx();
        let hud = build_hud(&ctx);
        assert!(hud.contains("UTC"));
        assert!(hud.contains("Ground Truth"));
    }

    #[test]
    fn test_lessons_section() {
        let lessons = vec![(0.9, "Never use unwrap".to_string()), (0.7, "Log before errors".to_string())];
        let out = format_lessons_section(&lessons);
        assert!(out.contains("0.9 conf"));
        assert!(out.contains("Never use unwrap"));
        assert!(out.contains("Log before errors"));
    }

    #[test]
    fn test_procedures_section() {
        let procs = vec!["git_commit".to_string(), "code_review".to_string()];
        let out = format_procedures_section(&procs);
        assert!(out.contains("SYSTEM NOTICE"));
        assert!(out.contains("git_commit, code_review"));
    }

    #[test]
    fn test_empty_sections_produce_nothing() {
        assert!(format_lessons_section(&[]).is_empty());
        assert!(format_procedures_section(&[]).is_empty());
        assert!(format_scratchpad_section(&None).is_empty());
        assert!(format_kg_section("").is_empty());
        assert!(format_reasoning_section(&[]).is_empty());
        assert!(format_steering_section(&None).is_empty());
        assert!(format_log_section("").is_empty());
        assert!(format_log_section("No recent errors.").is_empty());
        assert!(format_timeline_section("").is_empty());
        assert!(format_preferences_section(&None).is_empty());
    }

    #[test]
    fn test_hud_with_all_sections_populated() {
        let mut ctx = make_ctx();
        ctx.relevant_lessons = vec![(0.9, "Always verify".to_string())];
        ctx.relevant_procedures = vec!["deploy_flow".to_string()];
        ctx.scratchpad_content = Some("TODO: fix bug".to_string());
        ctx.kg_snapshot = "Recent: flux_server [service]".to_string();
        ctx.reasoning_traces = vec!["Investigated port issue".to_string()];
        ctx.active_steering = Some("curiosity (0.8)".to_string());
        ctx.system_log_tail = "[WARN] Flux 500".to_string();
        ctx.timeline_narrative = "[17:35] Started Flux".to_string();
        ctx.user_preferences = Some("Style: direct, no fluff".to_string());

        let hud = build_hud(&ctx);
        assert!(hud.contains("Active Lessons"));
        assert!(hud.contains("SYSTEM NOTICE"));
        assert!(hud.contains("Scratchpad (Active Notes)"));
        assert!(hud.contains("Knowledge Graph"));
        assert!(hud.contains("Recent Reasoning"));
        assert!(hud.contains("Active Steering"));
        assert!(hud.contains("System Logs"));
        assert!(hud.contains("Recent Timeline"));
        assert!(hud.contains("User Preferences"));
    }
}
