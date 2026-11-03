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
  yeet memory local catalog
  yeet memory local status MODEL
  yeet memory local install MODEL --download
  yeet memory local remove MODEL --confirm
  yeet memory local validate MODEL PROFILE_ID
  yeet memory local index MODEL PROFILE_ID
  yeet memory local recall MODEL PROFILE_ID QUERY

Text is stored locally; '-' reads stdin. Revision arguments prevent stale edits.
Local-only content is not exposed to models in this initial backend checkpoint.
Cloud probing sends only a bundled fixture, never your memories.
Managed inference is CPU-native and offline after installation.
Automatic agent capture/recall and visual management are not yet enabled.";

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
        ["local", rest @ ..] => local(&mut store, &access, rest),
        _ => bail!("invalid memory command; use `yeet memory help`"),
    }
}

fn local(store: &mut MemoryStore, access: &MemoryAccess, args: &[&str]) -> Result<()> {
    let models = ManagedModels::new(ConfigStore::default().directory.join("memory-models"));
    let cancel = AtomicBool::new(false);
    match args {
        ["catalog"] => print(&local_model_catalog()),
        ["status", model] => print(&models.status(model)?),
        ["install", model, "--download"] => {
            let mut last = 0;
            let status = models.install(model, &cancel, &mut |artifact, done, total| {
                if done == total || done.saturating_sub(last) > 4 * 1024 * 1024 {
                    eprintln!("{artifact}: {done}/{total} bytes");
                    last = done;
                }
            })?;
            print(&status)
        }
        ["remove", model, "--confirm"] => {
            models.remove(model)?;
            print(&models.status(model)?)
        }
        ["validate", model, profile] => {
            let encoder = LocalEmbedder::load(&models, model, (*profile).into())?;
            let fixture = vec![
                "A puppy plays with a dog.".into(),
                "A dog and puppy are playing.".into(),
                "Database transaction durability.".into(),
            ];
            let vectors = encoder.embed(&fixture, &cancel)?;
            let score = |a: usize, b: usize| {
                vectors[a]
                    .iter()
                    .zip(&vectors[b])
                    .map(|(x, y)| f64::from(*x) * f64::from(*y))
                    .sum::<f64>()
            };
            ensure!(
                score(0, 1) > score(0, 2),
                "local model retrieval fixture failed"
            );
            store.register_profile(access, encoder.profile())?;
            print(
                &serde_json::json!({"state":"ready","profile":encoder.profile(),"relatedSimilarity":score(0,1),"unrelatedSimilarity":score(0,2)}),
            )
        }
        ["index", model, profile] => {
            let encoder = LocalEmbedder::load(&models, model, (*profile).into())?;
            store.register_profile(access, encoder.profile())?;
            let mut filter = MemoryFilter {
                limit: 100,
                ..Default::default()
            };
            let mut indexed = 0;
            let mut blocked = Vec::new();
            loop {
                let page = store.list(access, &filter)?;
                for record in page.records {
                    let embedded = (|| -> Result<_> {
                        let chunks = encoder.chunks(&record.text)?;
                        let mut leaves = Vec::new();
                        for batch in chunks.chunks(64) {
                            let text: Vec<_> = batch.iter().map(|c| c.text.clone()).collect();
                            let vectors = encoder.embed(&text, &cancel)?;
                            leaves.extend(batch.iter().cloned().zip(vectors));
                        }
                        Ok(leaves)
                    })();
                    match embedded {
                        Ok(leaves) => {
                            store.put_vectors(
                                access,
                                &record.id,
                                record.revision,
                                encoder.profile(),
                                leaves,
                            )?;
                            indexed += 1;
                        }
                        Err(_) => blocked.push(record.id),
                    }
                }
                filter.after = page.next_cursor;
                if filter.after.is_none() {
                    break;
                }
            }
            print(
                &serde_json::json!({"indexed":indexed,"blocked":blocked,"note":"Complete model-aware leaf coverage; no text is silently truncated."}),
            )
        }
        ["recall", model, profile, query] => {
            let encoder = LocalEmbedder::load(&models, model, (*profile).into())?;
            let vectors = encoder.embed(&[(*query).into()], &cancel)?;
            print(&store.recall(access, query, Some((encoder.profile(), &vectors[0])), 10)?)
        }
        _ => bail!("invalid local model command; use `yeet memory help`"),
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
