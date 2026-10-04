//! Append-only session event log.
//!
//! Events are evidence, not state: each append takes the next per-session
//! sequence number and is written under the session lock; existing lines are
//! never rewritten.

use super::*;

const GROUP_CHECKPOINT_FILE: &str = "group-checkpoint.json";
const GROUP_CHECKPOINT_SCHEMA_VERSION: u64 = 1;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GroupCheckpointEnvelope<T> {
    schema_version: u64,
    checkpoint: T,
}

pub(super) fn event_is_debate(event: &serde_json::Value) -> bool {
    event
        .get("type")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|kind| kind.starts_with("debate"))
}

impl SessionStore {
    /// Persist a versioned Group Agent checkpoint in the session directory.
    pub fn save_group_checkpoint<T: Serialize>(&self, id: &str, checkpoint: &T) -> Result<()> {
        self.serialized_session(id, true, || {
            validate_id(id)?;
            let root = self.directory.join(id);
            create_private_dir(&root)?;
            let data = serde_json::to_vec(&serde_json::json!({
                "schemaVersion": GROUP_CHECKPOINT_SCHEMA_VERSION,
                "checkpoint": checkpoint,
            }))?;
            write_private_replace(&root.join(GROUP_CHECKPOINT_FILE), &data)
        })
    }

    /// Load a checkpoint, returning `None` for sessions created before this
    /// sidecar existed.
    pub fn load_group_checkpoint<T: serde::de::DeserializeOwned>(
        &self,
        id: &str,
    ) -> Result<Option<T>> {
        self.serialized_session(id, false, || {
            validate_id(id)?;
            let path = self.directory.join(id).join(GROUP_CHECKPOINT_FILE);
            let data = match fs::read(path) {
                Ok(data) => data,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error.into()),
            };
            let envelope: GroupCheckpointEnvelope<T> = serde_json::from_slice(&data)?;
            ensure!(
                envelope.schema_version == GROUP_CHECKPOINT_SCHEMA_VERSION,
                "unsupported Group Agent checkpoint schema version {}",
                envelope.schema_version
            );
            Ok(Some(envelope.checkpoint))
        })
    }

    /// Read the session's task and debate journal records in sequence order.
    /// Invalid lines are ignored so a partial final append cannot hide prior
    /// durable events.
    pub fn read_events(&self, id: &str) -> Result<Vec<serde_json::Value>> {
        self.serialized_session(id, false, || self.read_events_unlocked(id))
    }

    fn read_events_unlocked(&self, id: &str) -> Result<Vec<serde_json::Value>> {
        use std::io::{BufRead, BufReader};

        let root = self.directory.join(id);
        let mut events = Vec::new();
        for stream in [TASKS_DIR, DEBATES_DIR] {
            let path = root.join(stream).join("events.jsonl");
            let file = match fs::File::open(path) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            for line in BufReader::new(file).lines() {
                let line = line?;
                if let Ok(event) = serde_json::from_str::<serde_json::Value>(&line) {
                    if event
                        .get("seq")
                        .and_then(serde_json::Value::as_u64)
                        .is_some()
                    {
                        events.push(event);
                    }
                }
            }
        }
        events.sort_by_key(|event| {
            event
                .get("seq")
                .and_then(serde_json::Value::as_u64)
                .unwrap()
        });
        Ok(events)
    }

    pub fn append_event(
        &self,
        id: &str,
        run_id: Option<&str>,
        event: &serde_json::Value,
    ) -> Result<()> {
        self.serialized_session(id, true, || self.append_event_unlocked(id, run_id, event))
    }

    pub(super) fn append_event_unlocked(
        &self,
        id: &str,
        run_id: Option<&str>,
        event: &serde_json::Value,
    ) -> Result<()> {
        use std::io::Write;
        validate_id(id)?;
        let root = self.directory.join(id);
        create_private_dir(&root)?;
        let is_debate = event_is_debate(event);
        let category = if is_debate { DEBATES_DIR } else { TASKS_DIR };
        let event_directory = root.join(category);
        create_private_dir(&event_directory)?;
        let mut options = fs::OpenOptions::new();
        options.append(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let path = event_directory.join("events.jsonl");
        let seq = self.next_event_sequence(&root)?;
        let mut file = options.open(&path)?;
        let event_type = event
            .get("type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown")
            .to_owned();
        let mut payload = event.clone();
        if let Some(object) = payload.as_object_mut() {
            object.remove("type");
        }
        let record = serde_json::json!({
            "schemaVersion": 1,
            "seq": seq,
            "eventId": Uuid::new_v4().to_string(),
            "timestamp": Utc::now(),
            "sessionId": id,
            "runId": run_id,
            "stream": if is_debate { "debate" } else { "agent" },
            "type": event_type,
            "payload": payload,
        });
        writeln!(file, "{}", serde_json::to_string(&record)?)?;
        file.flush()?;
        Ok(())
    }

    pub(super) fn next_event_sequence(&self, root: &Path) -> Result<u64> {
        use std::io::{BufRead, BufReader};
        let counter_path = root.join(EVENT_SEQUENCE_FILE);
        let current = match fs::read_to_string(&counter_path) {
            Ok(value) => value.trim().parse::<u64>().unwrap_or(0),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut parsed_max = 0u64;
                let mut total_lines = 0u64;
                for stream in [TASKS_DIR, DEBATES_DIR] {
                    let path = root.join(stream).join("events.jsonl");
                    let Some(reader) = fs::File::open(path).ok().map(BufReader::new) else {
                        continue;
                    };
                    for line in reader.lines().map_while(Result::ok) {
                        total_lines = total_lines.saturating_add(1);
                        parsed_max = parsed_max.max(
                            serde_json::from_str::<serde_json::Value>(&line)
                                .ok()
                                .and_then(|value| value.get("seq")?.as_u64())
                                .unwrap_or(0),
                        );
                    }
                }
                parsed_max.max(total_lines)
            }
            Err(error) => return Err(error.into()),
        };
        let next = current.saturating_add(1);
        write_private_replace(&counter_path, next.to_string().as_bytes())?;
        Ok(next)
    }
}
