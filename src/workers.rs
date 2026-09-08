use std::{collections::HashMap, path::Path, sync::Arc};

use anyhow::{Result, bail};
use serde_json::{Map, Value};

use crate::core::ToolDefinition;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerDescriptor {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
}

pub trait HarnessWorker: Send + Sync {
    fn descriptor(&self) -> WorkerDescriptor;
    fn tools(&self) -> Vec<ToolDefinition>;
    fn execute(
        &self,
        tool_name: &str,
        arguments: &Map<String, Value>,
        workspace_root: &Path,
    ) -> Result<String>;
    fn cancel_task(&self, _task_id: &str) {}
    fn shutdown(&self) {}
}

#[derive(Default, Clone)]
pub struct WorkerRegistry {
    workers: Arc<HashMap<String, Arc<dyn HarnessWorker>>>,
}

impl WorkerRegistry {
    pub fn new(workers: Vec<Arc<dyn HarnessWorker>>) -> Result<Self> {
        let mut values = HashMap::new();
        for worker in workers {
            let descriptor = worker.descriptor();
            if descriptor.id.trim().is_empty() || values.contains_key(&descriptor.id) {
                bail!("duplicate or empty worker id: {}", descriptor.id);
            }
            values.insert(descriptor.id, worker);
        }
        Ok(Self {
            workers: Arc::new(values),
        })
    }

    pub fn descriptors(&self) -> Vec<WorkerDescriptor> {
        let mut values: Vec<_> = self
            .workers
            .values()
            .map(|worker| worker.descriptor())
            .collect();
        values.sort_by(|a, b| a.id.cmp(&b.id));
        values
    }

    pub fn activate(&self, id: &str) -> Result<Vec<ToolDefinition>> {
        self.workers
            .get(id)
            .map(|worker| worker.tools())
            .ok_or_else(|| anyhow::anyhow!("Unknown worker: {id}"))
    }

    pub fn execute(
        &self,
        id: &str,
        tool_name: &str,
        args: &Map<String, Value>,
        root: &Path,
    ) -> Result<String> {
        self.workers
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("Unknown worker: {id}"))?
            .execute(tool_name, args, root)
    }

    pub fn finish_task(&self, task_id: &str) {
        for worker in self.workers.values() {
            worker.cancel_task(task_id);
        }
    }

    pub fn shutdown(&self) {
        for worker in self.workers.values() {
            worker.shutdown();
        }
    }
}
