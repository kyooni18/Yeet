use super::*;
use crate::model::{
    AgentTaskItem, AuthProviderItem, BridgeState, ProviderUsageStatus, SessionActivity,
    SessionSummary,
};
use std::{path::Path, process::Command};

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn git_resources_cover_index_worktree_renames_binary_and_unborn_repositories() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().canonicalize().unwrap();
    git(&dir, &["init", "-q"]);
    std::fs::write(dir.join("first file\tname.rs"), "one\ntwo\n").unwrap();
    git(&dir, &["add", "."]);
    let unborn = GitSnapshot::load(&dir);
    assert!(unborn.message.is_none(), "{:?}", unborn.message);
    assert_eq!(
        unborn.changes[0].stats,
        Some(ChangeStats {
            added: 2,
            removed: 0
        })
    );
    assert!(
        GitSnapshot::patch(&dir.join("first file\tname.rs"))
            .unwrap()
            .iter()
            .any(|line| line == "+two")
    );
    std::fs::write(dir.join("delete.rs"), "deleted\n").unwrap();
    std::fs::write(dir.join("binary.dat"), [0, 1, 2]).unwrap();
    git(&dir, &["add", "."]);
    git(
        &dir,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-qm",
            "Fixture",
        ],
    );
    git(&dir, &["mv", "first file\tname.rs", "renamed\nfile.rs"]);
    std::fs::write(dir.join("renamed\nfile.rs"), "one\ntwo\nstaged\n").unwrap();
    git(&dir, &["add", "."]);
    std::fs::write(dir.join("renamed\nfile.rs"), "one\ntwo\nstaged\nworking\n").unwrap();
    std::fs::remove_file(dir.join("delete.rs")).unwrap();
    std::fs::write(dir.join("binary.dat"), [0, 3, 4]).unwrap();
    std::fs::write(dir.join("new file.rs"), "untracked\n").unwrap();
    let snapshot = GitSnapshot::load(&dir);
    assert!(snapshot.message.is_none(), "{:?}", snapshot.message);
    let find = |name: &str| {
        snapshot
            .changes
            .iter()
            .find(|change| change.path == Path::new(name))
            .unwrap()
    };
    let renamed = find("renamed\nfile.rs");
    assert_eq!(
        renamed.previous_path.as_deref(),
        Some(Path::new("first file\tname.rs"))
    );
    assert_eq!(
        renamed.stats,
        Some(ChangeStats {
            added: 4,
            removed: 2
        })
    );
    assert_eq!(find("binary.dat").stats, None);
    assert_eq!(
        find("delete.rs").stats,
        Some(ChangeStats {
            added: 0,
            removed: 1
        })
    );
    assert_eq!(
        find("new file.rs").stats,
        Some(ChangeStats {
            added: 1,
            removed: 0
        })
    );
    assert!(
        GitSnapshot::patch(&dir.join("delete.rs"))
            .unwrap()
            .iter()
            .any(|line| line == "-deleted")
    );
    assert!(
        GitSnapshot::patch(&dir.join("new file.rs"))
            .unwrap()
            .iter()
            .any(|line| line == "+untracked")
    );
}

#[test]
fn shared_content_preserves_real_activity_history_and_selection_across_updates() {
    let mut state = BridgeState {
        saved_sessions: vec![
            SessionSummary {
                id: "old".into(),
                title: "Old session".into(),
                updated_at: "2026-09-29T00:00:00Z".into(),
                model: "test/model".into(),
                message_count: 3,
            },
            SessionSummary {
                id: "new".into(),
                title: " Real\n session ".into(),
                updated_at: "2026-09-30T00:00:00Z".into(),
                model: "test/model".into(),
                message_count: 8,
            },
        ],
        ..BridgeState::default()
    };
    state
        .session_activity
        .insert("old".into(), SessionActivity::Running);
    state.agent_tasks.push(AgentTaskItem {
        id: "agent".into(),
        role: "Reviewer".into(),
        objective: "Check navigation".into(),
        status: "running".into(),
        summary: Some("Review in progress".into()),
    });
    state.auth_providers.push(AuthProviderItem {
        provider: "test".into(),
        authenticated: true,
        method: "oauth".into(),
        expires_at: None,
        error: None,
        usage: Some(ProviderUsageStatus {
            provider: "test".into(),
            available: false,
            source: "test".into(),
            fetched_at: String::new(),
            plan: None,
            windows: vec![],
            message: Some("No usage reported".into()),
        }),
    });
    let mut recent = RecentViews::default();
    let target = ResourceTarget::File("/workspace/src/home.rs".into());
    recent.visit(target.clone(), "home.rs");
    recent.visit(ResourceTarget::Session("new".into()), "Real session");
    recent.visit(target.clone(), "src/home.rs");
    assert_eq!(recent.entries().len(), 2);
    let visited = recent.entries()[0].visited_at;
    recent.update_title(&target, "Renamed title");
    assert_eq!(recent.entries()[0].visited_at, visited);
    let content = WorkspaceContent::collect(&state, &recent, &GitSnapshot::default());
    assert_eq!(content.sessions[0].title, "Real session");
    assert_eq!(content.sessions[1].context, "running");
    assert!(content.sessions[0].detail.contains("8 messages"));
    assert_eq!(content.tasks[0].context, "Reviewer · running");
    assert!(!content.providers[0].available);
    assert!(content.providers[0].windows.is_empty());
    let mut home = HomeState::default();
    home.replace_content(content.clone());
    home.selected = Some(target.clone());
    home.replace_content(content);
    assert_eq!(home.selected, Some(target));
    home.select_next(isize::MAX);
    assert_eq!(home.selected, Some(ResourceTarget::Task("agent".into())));
    home.replace_content(WorkspaceContent::default());
    assert!(home.selected.is_none());
}
