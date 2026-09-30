use super::*;

#[test]
fn runtime_lifecycle_and_relationships() -> anyhow::Result<()> {
    let registry = AgentRegistry::default();
    let parent = registry.register("parent", "model-a", "/workspace", None)?;
    let child = registry.register("child", "model-b", "/workspace", Some(parent))?;
    registry.connect(parent, child)?;
    registry.connect(parent, child)?;
    assert_eq!(registry.get(child)?.parent_agent, Some(parent));
    assert_eq!(registry.get(parent)?.coworkers, vec![child]);
    assert!(
        registry
            .register("invalid", "model", "/", Some(AgentId::new_v4()))
            .is_err()
    );
    assert!(registry.connect(parent, parent).is_err());
    {
        let _run = registry.begin_run(child, "model-c", "/other")?;
        assert_eq!(registry.get(child)?.status, AgentStatus::Running);
        assert_eq!(registry.get(child)?.model, "model-c");
        assert!(registry.begin_run(child, "model", "/").is_err());
        assert!(registry.unregister(child).is_err());
        registry.record_decision(
            child,
            AgentDecision {
                summary: "Completed".into(),
            },
        )?;
    }
    assert_eq!(registry.get(child)?.status, AgentStatus::Idle);
    assert_eq!(registry.get(child)?.decisions.len(), 1);
    registry.unregister(parent)?;
    assert_eq!(registry.get(child)?.parent_agent, None);
    assert!(registry.get(child)?.coworkers.is_empty());
    Ok(())
}

#[test]
fn run_guard_restores_idle_on_unwind() -> anyhow::Result<()> {
    let registry = AgentRegistry::default();
    let id = registry.register("agent", "model", "/", None)?;
    let result = std::panic::catch_unwind(|| {
        let _run = registry.begin_run(id, "model", "/").unwrap();
        panic!("simulated run failure");
    });
    assert!(result.is_err());
    assert_eq!(registry.get(id)?.status, AgentStatus::Idle);
    Ok(())
}
