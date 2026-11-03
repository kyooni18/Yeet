//! Local user-management commands for the native memory store. No model is
//! invoked by browsing, editing, deleting or searching canonical memory text.

use crate::{
    config::ConfigStore, core::BridgeClient, harness::memory::*, session_store::ensure_workspace_id,
};
use anyhow::{Result, bail, ensure};
use serde::Serialize;
use std::{
    fs,
    io::{self, Read},
    sync::atomic::AtomicBool,
};

const HELP: &str = "Native memory management (JSON output; current workspace):
  yeet memory list [QUERY]
  yeet memory search QUERY
  yeet memory get ID
  yeet memory save FILE|- [--local-only]
  yeet memory edit ID REVISION FILE|-
  yeet memory delete ID REVISION --confirm
  yeet memory privacy ID REVISION local-only
  yeet memory privacy ID REVISION shared --confirm-declassification
  yeet memory settings [FILE|-]
  yeet memory profiles
  yeet memory probe-cloud PROFILE_ID PROVIDER/MODEL --allow-cloud

Text is stored locally; '-' reads stdin. Revision arguments prevent stale edits.
Local-only content is not exposed to models in this initial backend checkpoint.
Cloud probing sends only a bundled fixture, never your memories.
Automatic capture/recall, managed inference and visual management are not yet enabled.";

pub(super) fn run(args: &[String]) -> Result<()> {
    let parts: Vec<&str> = args.iter().map(String::as_str).collect();
    if parts.is_empty() || parts == ["help"] || parts == ["--help"] {
        println!("{HELP}");
        return Ok(());
    }
    let workspace = std::env::current_dir()?.canonicalize()?;
    let id = ensure_workspace_id(&workspace)?;
    let access = MemoryAccess::local_management(id.clone())?;
    let mut store = MemoryStore::open(&ConfigStore::default().directory.join("memory"))?;
    match parts.as_slice() {
        ["list"] => print(&store.list(&access, &MemoryFilter::default())?),
        ["list" | "search", query] => print(&store.list(
            &access,
            &MemoryFilter {
                query: Some((*query).into()),
                ..Default::default()
            },
        )?),
        ["get", id] => print(&store.get(&access, id)?),
        ["save", file] | ["save", file, "--local-only"] => {
            let privacy = if parts.len() == 3 {
                Privacy::LocalOnly
            } else {
                Privacy::Shared
            };
            print(&store.save(
                &access,
                SaveMemory {
                    kind: MemoryKind::Explicit,
                    scope: MemoryScope::Workspace(id),
                    privacy,
                    text: read_text(file)?,
                    tags: vec![],
                    source: None,
                },
            )?)
        }
        ["edit", id, revision, file] => {
            print(&store.edit(&access, id, revision.parse()?, read_text(file)?, vec![])?)
        }
        ["delete", id, revision, "--confirm"] => {
            store.delete(&access, id, revision.parse()?)?;
            print(&serde_json::json!({"deleted":id}))
        }
        ["privacy", id, revision, "local-only"] => {
            print(&store.set_privacy(&access, id, revision.parse()?, Privacy::LocalOnly, false)?)
        }
        [
            "privacy",
            id,
            revision,
            "shared",
            "--confirm-declassification",
        ] => print(&store.set_privacy(&access, id, revision.parse()?, Privacy::Shared, true)?),
        ["settings"] => print(&store.settings(&id)?),
        ["settings", file] => {
            let settings: MemorySettings = serde_json::from_str(&read_text(file)?)?;
            // Operational enablement is stored now, but agent integration stays
            // gated independently; this command does not claim it is active.
            print(&store.update_settings(&access, settings)?)
        }
        ["profiles"] => print(&store.profiles()?),
        ["probe-cloud", profile, model, "--allow-cloud"] => {
            let bridge = BridgeClient::start_for_workspace(&workspace)?;
            let profile = probe_cloud_profile(
                &bridge,
                (*profile).into(),
                (*model).into(),
                &AtomicBool::new(false),
            )?;
            store.register_profile(&access, &profile)?;
            print(&profile)
        }
        _ => bail!("invalid memory command; use `yeet memory help`"),
    }
}

fn read_text(path: &str) -> Result<String> {
    let mut text = String::new();
    if path == "-" {
        io::stdin().take(128 * 1024 + 1).read_to_string(&mut text)?;
    } else {
        let file = fs::File::open(path)?;
        ensure!(
            file.metadata()?.len() <= 128 * 1024,
            "input exceeds 128 KiB"
        );
        file.take(128 * 1024 + 1).read_to_string(&mut text)?;
    }
    ensure!(text.len() <= 128 * 1024, "input exceeds 128 KiB");
    Ok(text)
}

fn print(value: &impl Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
