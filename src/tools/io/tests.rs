//! Regression tests for workspace I/O tools staying sidecar-free for read-only operations.

use super::*;

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
