use super::*;

#[test]
fn runtime_lifecycle_and_relationships() -> anyhow::Result<()> {
    let registry = AgentRegistry::default();
    let originator = registry.register("originator", "model-a", "/workspace", None)?;
    let spawned = registry.register("spawned", "model-b", "/workspace", Some(originator))?;
    registry.connect(originator, spawned)?;
    registry.connect(originator, spawned)?;
    assert_eq!(registry.get(spawned)?.spawned_by, Some(originator));
    assert_eq!(registry.get(originator)?.coworkers, vec![spawned]);
    assert!(
        registry
            .register("invalid", "model", "/", Some(AgentId::new_v4()))
            .is_err()
    );
    assert!(registry.connect(originator, originator).is_err());
    {
        let _run = registry.begin_run(spawned, "model-c", "/other")?;
        assert_eq!(registry.get(spawned)?.status, AgentStatus::Running);
        assert_eq!(registry.get(spawned)?.model, "model-c");
        assert!(registry.begin_run(spawned, "model", "/").is_err());
        assert!(registry.unregister(spawned).is_err());
        registry.record_decision(
            spawned,
            AgentDecision {
                summary: "Completed".into(),
            },
        )?;
    }
    assert_eq!(registry.get(spawned)?.status, AgentStatus::Idle);
    assert_eq!(registry.get(spawned)?.decisions.len(), 1);
    registry.unregister(originator)?;
    assert_eq!(registry.get(spawned)?.spawned_by, None);
    assert!(registry.get(spawned)?.coworkers.is_empty());
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
