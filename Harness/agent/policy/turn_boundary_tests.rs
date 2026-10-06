//! Tests for the parent module.

use super::*;
use crate::core::{ImageAttachment, ToolCall};

fn read_call(id: &str) -> Message {
    Message::assistant(
        "",
        Some(vec![ToolCall {
            id: id.into(),
            name: "web_read".into(),
            arguments: json!({"url":"https://example.com"}),
        }]),
    )
}

#[test]
fn tool_images_do_not_drop_prior_evidence_or_reorder_current_research() {
    let visual = Message::user_with_images(
        "Visual output returned by tool web_read.",
        vec![ImageAttachment {
            media_type: "image/png".into(),
            data: "image-data".into(),
            name: None,
        }],
    );
    let mut history = vec![
        Message::system(SYSTEM_INSTRUCTION),
        Message::user("old task"),
        read_call("old"),
        Message::tool("old evidence", "old", Some("web_read".into())),
        Message::user("current task"),
        read_call("first"),
        Message::tool("first evidence", "first", Some("web_read".into())),
        visual.clone(),
        Message::assistant("I will check the second source.", None),
        read_call("second"),
        Message::tool("second evidence", "second", Some("web_read".into())),
    ];
    let original = history.clone();
    for profile in [TaskProfile::Agent, TaskProfile::Research] {
        let request = request_history_for_profile_at(&history, profile, 4);
        let ids: Vec<_> = request
            .iter()
            .filter_map(|m| m.tool_call_id.as_deref())
            .collect();
        assert_eq!(ids, vec!["first", "second"]);
        let first = request
            .iter()
            .position(|m| m.tool_call_id.as_deref() == Some("first"))
            .unwrap();
        let image = request.iter().position(|m| m == &visual).unwrap();
        let commentary = request
            .iter()
            .position(|m| m.content.as_deref() == Some("I will check the second source."))
            .unwrap();
        let second = request
            .iter()
            .position(|m| m.tool_call_id.as_deref() == Some("second"))
            .unwrap();
        assert!(first < image && image < commentary && commentary < second);
        assert_eq!(request.iter().filter(|m| *m == &visual).count(), 1);
    }
    assert_eq!(history, original);
    history.push(Message::user("next task"));
    for profile in [TaskProfile::Agent, TaskProfile::Research] {
        let request = request_history_for_profile_at(&history, profile, history.len() - 1);
        assert!(request.iter().all(|m| m.role != MessageRole::Tool));
    }
}

#[test]
fn current_skill_and_rollover_handoff_survive_research_projection_only_for_current_turn() {
    let old_skill = Message::system("User-invoked Skill: old\nOLD");
    let current_skill = Message::system("User-invoked Skill: current\nCURRENT");
    let handoff = Message::system(
        "Internal context rollover handoff. successfulWorkspaceMutations=0; verification=not-run.",
    );
    let history = vec![
        Message::system(SYSTEM_INSTRUCTION),
        Message::user("old task"),
        old_skill.clone(),
        Message::assistant("old answer", None),
        Message::user("current task"),
        current_skill.clone(),
        handoff.clone(),
    ];

    let research = request_history_for_profile_at(&history, TaskProfile::Research, 4);
    assert!(!research.contains(&old_skill));
    assert_eq!(
        research
            .iter()
            .filter(|message| **message == current_skill)
            .count(),
        1
    );
    assert_eq!(
        research
            .iter()
            .filter(|message| **message == handoff)
            .count(),
        1
    );

    let agent = request_history_for_profile_at(&history, TaskProfile::Agent, 4);
    assert!(!agent.contains(&old_skill));
    assert_eq!(
        agent
            .iter()
            .filter(|message| **message == current_skill)
            .count(),
        1
    );
}

#[test]
fn natural_discovery_requests_route_to_web_without_hijacking_local_search() {
    assert!(looks_like_web_research_request(
        "search some performance enhancing mods of KSP."
    ));
    assert!(looks_like_web_research_request(
        "find me some current KSP performance mods"
    ));
    let a1b3 = "how different is Starship's entry profile compared to Space Shuttle";
    assert!(looks_like_web_research_request(a1b3));
    assert_eq!(
        task_profile_with_history(a1b3, true, &[], false),
        TaskProfile::Research
    );
    assert!(looks_like_web_research_request(
        "when will GPT-6 Astra will come into regular Chat mode"
    ));
    assert!(looks_like_web_research_request(
        "when is the new ChatGPT model available on Plus?"
    ));
    assert!(looks_like_web_research_request(
        "is GPT 6 Astra rolling out to ChatGPT Plus?"
    ));
    assert!(looks_like_web_research_request("GPT-6 Sol release rumors?"));
    assert_eq!(
        task_profile_with_history("GPT-6 Sol release rumors?", true, &[], false),
        TaskProfile::Research
    );
    assert!(!looks_like_web_research_request(
        "rumors about this repository build"
    ));
    assert!(!looks_like_web_research_request(
        "compare src/agent.rs to src/tools.rs"
    ));
    assert!(!looks_like_web_research_request(
        "search the repository for cacheSurfaceHash"
    ));
    assert!(!looks_like_web_research_request(
        "when will this build finish?"
    ));
    assert!(!looks_like_web_research_request(
        "when is the repository migration done?"
    ));
}

#[test]
fn capability_discovery_is_reserved_for_explicit_non_core_requests() {
    assert!(looks_like_capability_request("use the MCP browser tool"));
    assert!(looks_like_capability_request(
        "run this with the installed skill"
    ));
    assert!(looks_like_capability_request(
        "open the desktop with computer use"
    ));
    assert!(!looks_like_capability_request("inspect src/agent.rs"));
    assert!(!looks_like_capability_request("investigate high CPU usage"));
    assert!(!looks_like_capability_request(
        "analyze the latest shuttle log"
    ));
}

#[test]
fn prior_context_recovery_is_reserved_for_explicit_requests() {
    assert!(looks_like_prior_context_request("read the project memory"));
    assert!(looks_like_prior_context_request(
        "what did we do in the previous session?"
    ));
    assert!(!looks_like_prior_context_request(
        "inspect the current implementation"
    ));
    assert!(!looks_like_prior_context_request(
        "investigate high CPU usage"
    ));
}

#[test]
fn implicit_followup_preserves_web_surface_only_after_actual_web_use() {
    let followup = "has been that tested";
    let history = vec![
        Message::system(SYSTEM_INSTRUCTION),
        Message::user("Space Shuttle's RTLS abort sequence"),
        read_call("source"),
        Message::tool("NASA evidence", "source", Some("web_read".into())),
        Message::assistant("RTLS was part of the abort design.", None),
        Message::user(followup),
    ];
    assert_eq!(
        task_profile_with_history(followup, true, &history, false),
        TaskProfile::Agent
    );
    assert!(should_preserve_web_tool_surface(followup, &history));
    assert!(!should_preserve_web_tool_surface(
        "fix src/agent.rs",
        &history
    ));

    let no_web = vec![
        Message::system(SYSTEM_INSTRUCTION),
        Message::user("Explain this source"),
        Message::assistant("Explanation", None),
        Message::user("and this?"),
    ];
    assert!(!should_preserve_web_tool_surface("and this?", &no_web));
}

#[test]
fn research_budget_stops_simple_queries_and_preserves_deep_research_headroom() {
    assert_eq!(
        research_source_target("look up the current release date"),
        2
    );
    assert_eq!(research_source_target("recommend some KSP mods"), 3);
    assert_eq!(research_source_target("GPT-6 Sol rumors and leaks"), 3);
    assert_eq!(
        research_source_target("deep research all KSP performance mods"),
        6
    );

    let mut simple = ResearchBudget::for_input("recommend some KSP mods");
    simple.observe_tool(&web_read_call(0), "", true);
    simple.observe_tool(&web_read_call(1), "", true);
    assert!(!simple.sufficient(2));
    simple.observe_tool(&web_read_call(2), "", true);
    assert!(simple.sufficient(2));
    assert!(ResearchBudget::for_input("recommend some KSP mods").sufficient(6));

    let mut deep = ResearchBudget::for_input("deep research all KSP performance mods");
    for index in 0..3 {
        deep.observe_tool(&web_read_call(index), "", true);
    }
    assert!(!deep.sufficient(4));
    for index in 3..6 {
        deep.observe_tool(&web_read_call(index), "", true);
    }
    assert!(deep.sufficient(4));
    assert!(ResearchBudget::for_input("deep research all KSP performance mods").sufficient(10));
}

#[test]
fn research_history_is_token_bounded_but_keeps_more_short_context() {
    let mut short = vec![Message::system(SYSTEM_INSTRUCTION)];
    for index in 0..10 {
        short.push(Message::user(format!("short question {index}")));
        short.push(Message::assistant(format!("short answer {index}"), None));
    }
    short.push(Message::user("current"));
    let request = request_history_for_profile_at(&short, TaskProfile::Research, short.len() - 1);
    // Sixteen bounded ordinary messages are allowed when they are cheap;
    // the old fixed seven-message window discarded useful short context.
    assert_eq!(
        request
            .iter()
            .filter(|message| {
                matches!(message.role, MessageRole::User | MessageRole::Assistant)
                    && message.content.as_deref() != Some("current")
            })
            .count(),
        16
    );

    let mut large = vec![Message::system(SYSTEM_INSTRUCTION)];
    for index in 0..10 {
        large.push(Message::user(format!("{index}:{}", "x".repeat(12_000))));
        large.push(Message::assistant(
            format!("{index}:{}", "y".repeat(12_000)),
            None,
        ));
    }
    large.push(Message::user("current"));
    let request = request_history_for_profile_at(&large, TaskProfile::Research, large.len() - 1);
    let ordinary = request
        .iter()
        .filter(|message| {
            matches!(message.role, MessageRole::User | MessageRole::Assistant)
                && message.content.as_deref() != Some("current")
        })
        .cloned()
        .collect::<Vec<_>>();
    assert!(ordinary.len() < 7);
    assert!(
        serde_json::to_vec(&ordinary)
            .map(|bytes| (bytes.len() as u64).div_ceil(3) <= 12_000)
            .unwrap()
    );
}

fn web_read_call(index: usize) -> ToolCall {
    ToolCall {
        id: format!("read-{index}"),
        name: "web_read".into(),
        arguments: json!({ "url": format!("https://example.com/source-{index}") }),
    }
}
