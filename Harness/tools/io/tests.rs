//! Regression tests for workspace I/O tools staying sidecar-free for read-only operations.

use super::*;

#[test]
fn active_sessions_can_edit_the_same_file_without_finishing_their_tasks() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    fs::write(root.join("shared.txt"), "one\ntwo\n").unwrap();
    let registry = || {
        ToolRegistry::new_with_bridge_handle(
            BridgeHandle::lazy_for_workspace(root.clone()),
            root.clone(),
            WorkerRegistry::new(Vec::new()).unwrap(),
            PermissionBroker::default(),
        )
        .unwrap()
    };
    let mut first = registry();
    let mut second = registry();
    first.begin_task("task-a");
    second.begin_task("task-b");
    let read = json!({"path":"shared.txt","startLine":1,"endLine":2});
    first.read_file(read.as_object().unwrap()).unwrap();
    second.read_file(read.as_object().unwrap()).unwrap();
    let change = |line, text| {
        json!({
            "diagnostics": false,
            "changes": [{"path":"shared.txt", "edits":[{
                "kind":"replace", "range":{"start":line,"end":line}, "text":text
            }]}]
        })
    };
    first.apply_file_edits(&change(1, "ONE")).unwrap();
    // Another session's old local read must not silently overwrite live edits.
    let stale = second.apply_file_edits(&change(1, "obsolete")).unwrap_err();
    assert!(
        stale.to_string().contains("changed after it was read"),
        "{stale}"
    );
    second.read_file(read.as_object().unwrap()).unwrap();
    second.apply_file_edits(&change(2, "TWO")).unwrap();
    first.read_file(read.as_object().unwrap()).unwrap();
    first.apply_file_edits(&change(1, "FINAL")).unwrap();
    assert_eq!(
        fs::read_to_string(root.join("shared.txt")).unwrap(),
        "FINAL\nTWO\n"
    );
    first.finish_task("task-a");
    second.finish_task("task-b");
}

#[test]
fn read_only_workspace_tools_do_not_start_sidecars() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
    fs::write(root.join("src/example.rs"), "hello world\nsecond line\n").unwrap();
    fs::write(root.join("node_modules/pkg/hidden.js"), "hello hidden\n").unwrap();

    let workers = WorkerRegistry::new(Vec::new()).unwrap();
    let mut registry = ToolRegistry::new_with_bridge_handle(
        BridgeHandle::lazy_for_workspace(root.clone()),
        root,
        workers,
        PermissionBroker::default(),
    )
    .unwrap();

    let read = json!({"path":"src/example.rs","startLine":1,"endLine":1});
    let read = registry.read_file(read.as_object().unwrap()).unwrap();
    assert!(read.contains("hello world"), "{read}");
    assert!(!registry.bridge.is_started());
    assert!(!registry.is_edit_started());

    let list = json!({"path":".","maxDepth":3,"maxResults":100});
    let list = registry.list_files(list.as_object().unwrap()).unwrap();
    assert!(list.contains("src/example.rs"), "{list}");
    assert!(!list.contains("node_modules"), "{list}");
    assert!(!registry.bridge.is_started());
    assert!(!registry.is_edit_started());

    let search = json!({"path":".","query":"hel.o world","regex":true});
    let search = registry
        .search_workspace(search.as_object().unwrap())
        .unwrap();
    assert!(search.contains("src/example.rs"), "{search}");
    assert!(!search.contains("hidden.js"), "{search}");
    assert!(!registry.bridge.is_started());
    assert!(!registry.is_edit_started());

    let generated = json!({"path":"node_modules"});
    assert!(registry.list_files(generated.as_object().unwrap()).is_err());
}

#[test]
fn shell_directory_persists_between_commands_within_context() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    fs::create_dir(root.join("sub")).unwrap();
    let store = crate::harness::sandbox::SandboxStore::new(&root).unwrap();
    let mut policy = store.load().unwrap();
    policy.workspace_read = crate::harness::sandbox::WorkspaceRead::All;
    store.save(&policy).unwrap();
    let mut registry = ToolRegistry::new_with_bridge_handle(
        BridgeHandle::lazy_for_workspace(root.clone()),
        root.clone(),
        WorkerRegistry::new(Vec::new()).unwrap(),
        PermissionBroker::default(),
    )
    .unwrap();
    let cancel = AtomicBool::new(false);
    let mut run = |command: &str| {
        let args = json!({ "command": command, "mode": "direct" });
        registry
            .run_shell_tool(args.as_object().unwrap(), "", &cancel)
            .unwrap()
    };

    let moved = run("cd sub");
    assert!(moved.contains("shellCwd"), "{moved}");
    let inside = run("pwd -P");
    let value: Value = serde_json::from_str(&inside).unwrap();
    assert_eq!(
        value["stdout"].as_str().unwrap().trim(),
        root.join("sub").display().to_string()
    );
    assert!(!inside.contains("__YEET_SHELL_CWD__"), "{inside}");

    // Leaving the context roots does not persist.
    run("cd /");
    let reset = run("pwd -P");
    let value: Value = serde_json::from_str(&reset).unwrap();
    assert_eq!(
        value["stdout"].as_str().unwrap().trim(),
        root.display().to_string()
    );
}

#[test]
fn complete_actor_output_is_not_marked_truncated() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let mut registry = ToolRegistry::new_with_bridge_handle(
        BridgeHandle::lazy_for_workspace(root.clone()),
        root,
        WorkerRegistry::new(Vec::new()).unwrap(),
        PermissionBroker::default(),
    )
    .unwrap();
    let args = json!({ "command": "echo hi", "mode": "actor" });
    let result = registry
        .run_shell_tool(args.as_object().unwrap(), "", &AtomicBool::new(false))
        .unwrap();
    assert!(!result.contains("artifactId"), "{result}");
}

#[test]
fn large_shell_output_keeps_real_head_and_tail() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let mut registry = ToolRegistry::new_with_bridge_handle(
        BridgeHandle::lazy_for_workspace(root.clone()),
        root,
        WorkerRegistry::new(Vec::new()).unwrap(),
        PermissionBroker::default(),
    )
    .unwrap();
    for mode in ["direct", "actor"] {
        let args = json!({ "command": "seq 1 20000", "mode": mode });
        let result = registry
            .run_shell_tool(args.as_object().unwrap(), "", &AtomicBool::new(false))
            .unwrap();
        let value: Value = serde_json::from_str(&result).unwrap();
        let stdout = value["stdout"].as_str().unwrap();
        assert!(stdout.starts_with("1\n2\n"), "{mode}: {stdout:.80}");
        assert!(stdout.ends_with("19999\n20000\n"), "{mode}");
        assert!(stdout.contains("bytes omitted"), "{mode}");
        assert!(value["artifactId"].is_string(), "{mode}");
    }
}
