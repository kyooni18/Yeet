use std::{path::Path, thread, time::Duration};

use anyhow::Result;
use yeet::harness::{Harness, HarnessCommand};

fn attach_yeet(workspace: &Path) -> Result<Harness> {
    // Use `Harness::attach(workspace)` instead when this host should share the
    // workspace daemon/session with other Yeet frontends.
    Harness::embedded(workspace)
}

fn main() -> Result<()> {
    let workspace = std::env::current_dir()?;
    let mut harness = attach_yeet(&workspace)?;

    harness.send(HarnessCommand::RequestModels)?;
    while let Some(event) = harness.try_recv() {
        if let Some(state) = event.state {
            println!("{} model(s) available", state.available_models.len());
            return Ok(());
        }
    }

    // A GUI/service host would normally integrate this with its own event loop.
    thread::sleep(Duration::from_millis(10));
    Ok(())
}
