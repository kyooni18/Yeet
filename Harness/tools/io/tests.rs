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
    first
        .apply_file_edits(&change(1, "ONE"), &AtomicBool::new(false))
        .unwrap();
    // Another session's old local read must not silently overwrite live edits.
    let stale = second
        .apply_file_edits(&change(1, "obsolete"), &AtomicBool::new(false))
        .unwrap_err();
    assert!(
        stale.to_string().contains("changed after it was read"),
        "{stale}"
    );
    second.read_file(read.as_object().unwrap()).unwrap();
    second
        .apply_file_edits(&change(2, "TWO"), &AtomicBool::new(false))
        .unwrap();
    first.read_file(read.as_object().unwrap()).unwrap();
    first
        .apply_file_edits(&change(1, "FINAL"), &AtomicBool::new(false))
        .unwrap();
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
        root.join("sub").display().to_string(),
        "tool result: {inside}"
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
fn long_foreground_command_hands_off_instead_of_dying() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let mut registry = ToolRegistry::new_with_bridge_handle(
        BridgeHandle::lazy_for_workspace(root.clone()),
        root,
        WorkerRegistry::new(Vec::new()).unwrap(),
        PermissionBroker::default(),
    )
    .unwrap();
    // A hard execution deadline must not extend the foreground yield window.
    let args = json!({ "command": "sleep 30", "yieldSeconds": 1, "timeoutSeconds": 20 });
    let value: Value = serde_json::from_str(
        &registry
            .run_shell_tool(args.as_object().unwrap(), "", &AtomicBool::new(false))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(value["status"], json!("running"), "{value}");
    assert_eq!(value["outputHandedOff"], json!(true));
    let job_id = value["jobId"].as_str().unwrap();

    let stop = json!({ "action": "stop", "jobId": job_id });
    registry
        .shell_job_tool(stop.as_object().unwrap(), &AtomicBool::new(false))
        .unwrap();
    let wait = json!({ "action": "wait", "jobId": job_id });
    let value: Value = serde_json::from_str(
        &registry
            .shell_job_tool(wait.as_object().unwrap(), &AtomicBool::new(false))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(value["status"], json!("cancelled"), "{value}");
}

#[cfg(unix)]
#[test]
fn shell_job_tool_controls_pty() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let mut registry = ToolRegistry::new_with_bridge_handle(
        BridgeHandle::lazy_for_workspace(root.clone()),
        root,
        WorkerRegistry::new(Vec::new()).unwrap(),
        PermissionBroker::default(),
    )
    .unwrap();
    let cancel = AtomicBool::new(false);
    let args = json!({"command":"stty -echo; printf ready; read line; stty size; printf 'got:%s\\n' \"$line\"; sleep 30", "pty":true, "background":true});
    let result: Value = serde_json::from_str(
        &registry
            .run_shell_tool(args.as_object().unwrap(), "", &cancel)
            .unwrap(),
    )
    .unwrap();
    let id = result["jobId"].as_str().unwrap();
    let started = std::time::Instant::now();
    loop {
        let output = json!({"action":"output", "jobId":id, "offset":0});
        let page: Value = serde_json::from_str(
            &registry
                .shell_job_tool(output.as_object().unwrap(), &cancel)
                .unwrap(),
        )
        .unwrap();
        if !page["events"].as_array().unwrap().is_empty() {
            break;
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "PTY never became ready"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    for request in [
        json!({"action":"resize", "jobId":id, "rows":41, "cols":99}),
        json!({"action":"input", "jobId":id, "data":"hello\n"}),
    ] {
        registry
            .shell_job_tool(request.as_object().unwrap(), &cancel)
            .unwrap();
    }
    loop {
        let check = json!({"action":"check", "jobId":id});
        let state: Value = serde_json::from_str(
            &registry
                .shell_job_tool(check.as_object().unwrap(), &cancel)
                .unwrap(),
        )
        .unwrap();
        if state["progress"]["stdoutTail"]
            .as_str()
            .unwrap_or_default()
            .contains("got:hello")
        {
            break;
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "input was not received: {state}"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let interrupt = json!({"action":"interrupt", "jobId":id});
    registry
        .shell_job_tool(interrupt.as_object().unwrap(), &cancel)
        .unwrap();
    let wait = json!({"action":"wait", "jobId":id});
    let result: Value = serde_json::from_str(
        &registry
            .shell_job_tool(wait.as_object().unwrap(), &cancel)
            .unwrap(),
    )
    .unwrap();
    assert_ne!(result["status"], "running");
    let stdout = result["stdout"].as_str().unwrap_or_default();
    assert!(
        stdout.contains("41 99") && stdout.contains("got:hello"),
        "{result}"
    );
    assert!(!stdout.contains("Operation not permitted"), "{result}");

    // The local frontend can rebind one registry to another session. A clone
    // already held by the frontend must no longer resolve the old job ID.
    let access = registry.shell_job_access();
    let store = SessionStore::new(directory.path());
    registry.set_session_runtime(store.clone(), Some("first".into()));
    assert!(access.execute(id, "output", 0, "", 0, 0).is_ok());
    registry.set_session_runtime(store, Some("second".into()));
    assert!(access.execute(id, "output", 0, "", 0, 0).is_err());
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
    let value: Value = serde_json::from_str(&result).unwrap();
    assert_eq!(value["stdout"], "hi\n");
    assert_eq!(value["stdoutTruncated"], false);
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
        assert_eq!(value["stdoutBytes"], json!(108894), "{mode}");
        assert_eq!(value["outputWindowed"], json!(true), "{mode}");
        let truncated = value["stdoutTruncated"].as_bool().unwrap();
        assert_eq!(value["artifactComplete"], json!(true), "{mode}");
        let id = value["artifactId"].as_str().unwrap();
        let middle = registry
            .artifacts
            .read_page(id, Some(10000), Some(10002), 0, 8192)
            .unwrap();
        assert_eq!(middle, "10000\n10001\n10002");
        let transcript = fs::read_to_string(registry.artifacts.path(id)).unwrap();
        assert_eq!(transcript.len(), 108894);
        assert!(!transcript.contains("__YEET_SHELL_CWD__"));
        if mode == "direct" {
            assert!(
                truncated,
                "capture loss must not be hidden behind an artifact"
            );
        }
    }
}

#[test]
fn text_replacements_and_patches_edit_files_without_line_numbers() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    fs::write(
        root.join("lib.rs"),
        "fn a() {\n    let x = 1;\n}\n\nfn b() {\n    let x = 1;\n}\n",
    )
    .unwrap();
    fs::write(root.join("old.txt"), "gone\n").unwrap();
    let mut registry = ToolRegistry::new_with_bridge_handle(
        BridgeHandle::lazy_for_workspace(root.clone()),
        root.clone(),
        WorkerRegistry::new(Vec::new()).unwrap(),
        PermissionBroker::default(),
    )
    .unwrap();
    registry.begin_task("task");
    let read = |registry: &mut ToolRegistry, path: &str| {
        let request = json!({"path": path});
        registry.read_file(request.as_object().unwrap()).unwrap();
    };
    read(&mut registry, "lib.rs");
    let replace = |old: &str, new: &str| {
        json!({"diagnostics": false, "changes": [{"path": "lib.rs", "edits": [
            {"kind": "replaceText", "oldText": old, "newText": new}
        ]}]})
    };
    let ambiguous = registry
        .apply_file_edits(
            &replace("let x = 1;", "let x = 2;"),
            &AtomicBool::new(false),
        )
        .unwrap_err();
    assert!(
        ambiguous.to_string().contains("matches 2 places"),
        "{ambiguous}"
    );
    registry
        .apply_file_edits(
            &replace("fn a() {\n    let x = 1;", "fn a() {\n    let x = 2;"),
            &AtomicBool::new(false),
        )
        .unwrap();

    read(&mut registry, "lib.rs");
    read(&mut registry, "old.txt");
    let patch = "*** Begin Patch\n*** Update File: lib.rs\n@@ fn b() {\n-    let x = 1;\n+    let y = 3;\n+    let z = 4;\n }\n*** Add File: new.txt\n+hello\n*** Delete File: old.txt\n*** End Patch\n";
    registry
        .apply_file_edits(
            &json!({"diagnostics": false, "patch": patch}),
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(
        fs::read_to_string(root.join("lib.rs")).unwrap(),
        "fn a() {\n    let x = 2;\n}\n\nfn b() {\n    let y = 3;\n    let z = 4;\n}\n"
    );
    assert_eq!(fs::read_to_string(root.join("new.txt")).unwrap(), "hello\n");
    assert!(!root.join("old.txt").exists());
    registry.finish_task("task");
}

#[test]
fn edits_chain_without_rereads_until_the_file_changes_elsewhere() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    fs::write(root.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    let mut registry = ToolRegistry::new_with_bridge_handle(
        BridgeHandle::lazy_for_workspace(root.clone()),
        root.clone(),
        WorkerRegistry::new(Vec::new()).unwrap(),
        PermissionBroker::default(),
    )
    .unwrap();
    registry.begin_task("task");
    let read = json!({"path": "a.txt"});
    registry.read_file(read.as_object().unwrap()).unwrap();
    let replace = |old: &str, new: &str| {
        json!({"diagnostics": false, "changes": [{"path": "a.txt", "edits": [
            {"kind": "replaceText", "oldText": old, "newText": new}
        ]}]})
    };
    registry
        .apply_file_edits(&replace("one", "ONE"), &AtomicBool::new(false))
        .unwrap();
    // Our own edit and an unknown-scope shell mutation keep the snapshot.
    registry.invalidate_workspace_cache();
    registry
        .apply_file_edits(&replace("two", "TWO"), &AtomicBool::new(false))
        .unwrap();
    assert_eq!(
        fs::read_to_string(root.join("a.txt")).unwrap(),
        "ONE\nTWO\nthree\n"
    );

    fs::write(root.join("a.txt"), "ONE\nTWO\nchanged\n").unwrap();
    registry.invalidate_workspace_cache();
    let stale = registry
        .apply_file_edits(&replace("ONE", "uno"), &AtomicBool::new(false))
        .unwrap_err();
    assert!(
        stale.to_string().contains("changed after it was read"),
        "{stale}"
    );
    registry.finish_task("task");
}

#[test]
fn foreground_shell_reports_real_partial_output_while_it_runs() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
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
    let mut updates = Vec::new();
    let args = json!({ "command": "printf first; sleep 1.5; printf second", "mode": "direct" });
    let result = registry
        .run_shell_tool_live(
            args.as_object().unwrap(),
            "",
            &AtomicBool::new(false),
            &mut |payload| updates.push(payload),
        )
        .unwrap();
    let final_value: Value = serde_json::from_str(&result).unwrap();
    assert_eq!(final_value["stdout"], "firstsecond");
    let live: Value =
        serde_json::from_str(updates.first().expect("progress while running")).unwrap();
    assert_eq!(live["status"], "running");
    assert_eq!(live["progress"]["stdoutTail"], "first");
    // Nothing is reported for a stream that has produced no bytes.
    assert!(live["progress"]["stderrTail"].is_null());
}
