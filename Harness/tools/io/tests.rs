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
