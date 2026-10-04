//! File observations are one adapter of the provider-neutral cache planner.

use std::{collections::HashMap, time::Instant};

use serde_json::{Value, json};

use crate::{
    context_cache::{InputRates, RevisionJournal, UpdateEstimate, UpdateStrategy, plan_update},
    core::{Message, MessageRole, Usage},
};

struct Observation {
    key: String,
    revision: String,
    message_index: usize,
    original_content: String,
    observed_at: Instant,
}

pub(in crate::agent) struct ObservationPolicy {
    pub prefix_overhead_tokens: u64,
    pub may_retire: bool,
}

/// Ledger entries never change. Current history is a derived view; retiring an
/// obsolete body leaves the original observation in this ledger and tool events.
pub(in crate::agent) struct ObservationCache {
    journal: RevisionJournal<Observation>,
    pricing: HashMap<String, Option<Value>>,
    cache_hit_fraction: Option<f64>,
    last_submission: Instant,
    plans: Vec<Value>,
    window: Option<String>,
}

impl Default for ObservationCache {
    fn default() -> Self {
        Self {
            journal: RevisionJournal::default(),
            pricing: HashMap::new(),
            cache_hit_fraction: None,
            last_submission: Instant::now(),
            plans: Vec::new(),
            window: None,
        }
    }
}

fn observation_identity(value: &Value) -> Option<(String, String)> {
    if value.get("duplicate").and_then(Value::as_bool) == Some(true)
        || value.get("externalized").and_then(Value::as_bool) == Some(true)
    {
        return None;
    }
    let path = value.get("path")?.as_str()?;
    let snapshot = value.get("snapshot")?.as_str()?;
    // Only exact, fully returned read ranges are interchangeable. Partial,
    // externalized and batched observations keep their historical provenance.
    value.get("lines")?.as_str()?;
    let start = value.get("startLine")?.as_u64()?;
    let end = value.get("endLine")?.as_u64()?;
    Some((format!("file:{path}:{start}:{end}"), snapshot.to_owned()))
}

fn tokens(content: &str) -> u64 {
    (content.len() as u64).div_ceil(3)
}

fn rates(pricing: &Value, input_tokens: u64) -> Option<InputRates> {
    if pricing.get("currency")?.as_str()? != "USD"
        || pricing.get("unit")?.as_str()? != "per1MTokens"
    {
        return None;
    }
    let mut effective = pricing;
    if let Some(tiers) = pricing.get("tiers").and_then(Value::as_array) {
        for tier in tiers {
            if tier
                .get("thresholdTokens")
                .and_then(Value::as_u64)
                .is_some_and(|limit| input_tokens > limit)
            {
                effective = tier;
            }
        }
    }
    Some(InputRates {
        uncached: effective.get("input")?.as_f64()?,
        cached: effective.get("cacheRead")?.as_f64()?,
        cache_write: effective.get("cacheWrite").and_then(Value::as_f64),
    })
}

impl ObservationCache {
    pub(in crate::agent) fn bind_window(&mut self, window: &str) {
        if self.window.as_deref() != Some(window) {
            self.journal = RevisionJournal::default();
            self.plans.clear();
            self.cache_hit_fraction = None;
            self.window = Some(window.to_owned());
        }
    }

    pub(in crate::agent) fn start_turn(&mut self) {
        self.pricing.clear();
        self.cache_hit_fraction = None;
    }

    pub(in crate::agent) fn observe_usage(&mut self, usage: Option<&Usage>) {
        self.cache_hit_fraction = usage.and_then(|usage| usage.cache_measurement().hit_rate);
    }

    pub(in crate::agent) fn mark_submitted(&mut self) {
        self.last_submission = Instant::now();
    }

    pub(in crate::agent) fn take_plans(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.plans)
    }

    /// Append the correction. If economics justify it, shorten only an obsolete
    /// read body; keep message roles, IDs, tool-call pairs and chronological facts.
    pub(in crate::agent) fn prepare_read_update<F: FnOnce() -> Option<Value>>(
        &mut self,
        history: &mut [Message],
        incoming: &mut String,
        model: &str,
        policy: ObservationPolicy,
        load_pricing: F,
    ) -> bool {
        let Ok(mut value) = serde_json::from_str::<Value>(incoming) else {
            return false;
        };
        let Some((key, revision)) = observation_identity(&value) else {
            return false;
        };
        let prior = history
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, message)| {
                if message.role != MessageRole::Tool || message.name.as_deref() != Some("read_file")
                {
                    return None;
                }
                let content = message.content.as_deref()?;
                let old: Value = serde_json::from_str(content).ok()?;
                let (old_key, old_revision) = observation_identity(&old)?;
                (old_key == key).then_some((index, content.to_owned(), old_revision))
            });
        let mut retired = false;
        if let Some((index, old_content, old_revision)) = prior
            && old_revision != revision
        {
            let now = Instant::now();
            let age = self
                .journal
                .records()
                .iter()
                .rev()
                .find(|entry| {
                    entry.key == key
                        && entry.revision == old_revision
                        && entry.message_index == index
                        && entry.original_content == old_content
                })
                .map(|entry| now.duration_since(entry.observed_at));
            value["supersedesSnapshot"] = json!(old_revision);
            value["observationAgeSeconds"] = json!(age.map(|age| age.as_secs_f64()));
            value["updateHint"] = json!(
                "New observation: this snapshot supersedes earlier source for this exact range. Earlier tool outcomes are historical facts, not the current file state."
            );
            *incoming = value.to_string();
            let marker = json!({
                "supersededObservation": true, "snapshot": old_revision,
                "replacementMessageIndex": history.len(),
                "hint": "This historical read body is superseded. Read the fresh observation at replacementMessageIndex; retain the original tool-call outcome as a historical fact."
            }).to_string();
            let total = super::super::context::estimate_messages(history)
                .unwrap_or(u64::MAX)
                .saturating_add(policy.prefix_overhead_tokens);
            let suffix =
                super::super::context::estimate_messages(&history[index..]).unwrap_or(total);
            // Fetch only on a changed observation, once per model per turn.
            // Missing metadata is a reason to append, never to guess prices.
            let pricing = self
                .pricing
                .entry(model.to_owned())
                .or_insert_with(load_pricing);
            let input_rates = pricing
                .as_ref()
                .and_then(|pricing| rates(pricing, total.saturating_add(tokens(incoming))));
            let replacement_tokens = total
                .saturating_sub(tokens(&old_content))
                .saturating_add(tokens(&marker))
                .saturating_add(tokens(incoming));
            let retire_rates = pricing
                .as_ref()
                .and_then(|pricing| rates(pricing, replacement_tokens));
            // One prospective reuse is conservative. The generic planner also
            // accepts longer horizons and provider-specific retention lifetimes.
            let plan = plan_update(UpdateEstimate {
                context_tokens: total,
                suffix_tokens: suffix,
                obsolete_tokens: tokens(&old_content),
                correction_tokens: tokens(incoming),
                reference_tokens: tokens(&marker),
                observation_age: age,
                cache_idle_age: now.duration_since(self.last_submission),
                cache_ttl: None,
                cache_hit_fraction: self.cache_hit_fraction,
                expected_reuses: 1,
                rates: input_rates,
                retire_rates,
                may_retire_obsolete_body: policy.may_retire && retire_rates.is_some(),
            });
            retired = plan.strategy == UpdateStrategy::RetireObsoleteBody;
            self.plans.push(json!({"key":key, "previousMessageIndex":index, "tokenEstimate":"utf8_bytes_div_3", "plan":plan}));
            if retired {
                history[index].content = Some(marker);
            }
        }
        self.journal.append(Observation {
            key,
            revision,
            message_index: history.len(),
            original_content: incoming.clone(),
            observed_at: Instant::now(),
        });
        retired
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corrections_preserve_prefix_or_retire_only_obsolete_body_with_original_in_ledger() {
        let pricing =
            json!({"currency":"USD", "unit":"per1MTokens", "input":10.0, "cacheRead":1.0});
        for (hit, expected_retirement) in [(None, false), (Some(1.0), false), (Some(0.0), true)] {
            let mut cache = ObservationCache::default();
            let mut history = vec![Message::system("base")];
            let mut first = json!({"path":"a.txt", "snapshot":"old", "startLine":1, "endLine":1, "lines":"x".repeat(6000)}).to_string();
            assert!(!cache.prepare_read_update(
                &mut history,
                &mut first,
                "test/model",
                ObservationPolicy {
                    prefix_overhead_tokens: 0,
                    may_retire: true
                },
                || None
            ));
            history.push(Message::tool(
                first.clone(),
                "call-a",
                Some("read_file".into()),
            ));
            history.push(Message::assistant(
                "unrelated subsequent evidence".repeat(20000),
                None,
            ));
            let before = history.clone();
            cache.cache_hit_fraction = hit;
            let mut fresh = json!({"path":"a.txt", "snapshot":"new", "startLine":1, "endLine":1, "lines":"new source"}).to_string();
            assert_eq!(
                cache.prepare_read_update(
                    &mut history,
                    &mut fresh,
                    "test/model",
                    ObservationPolicy {
                        prefix_overhead_tokens: 0,
                        may_retire: true
                    },
                    || Some(pricing.clone())
                ),
                expected_retirement
            );
            assert_eq!(history[0], before[0]);
            assert_eq!(history[2], before[2]);
            assert_eq!(history[1].tool_call_id, before[1].tool_call_id);
            if expected_retirement {
                assert!(
                    history[1]
                        .content
                        .as_deref()
                        .unwrap()
                        .contains("replacementMessageIndex")
                );
            } else {
                assert_eq!(history, before);
            }
            assert_eq!(cache.journal.records()[0].original_content, first);
            let correction: Value = serde_json::from_str(&fresh).unwrap();
            assert_eq!(correction["supersedesSnapshot"], "old");
            assert!(correction["observationAgeSeconds"].as_f64().is_some());
            history.push(Message::tool(fresh, "call-b", Some("read_file".into())));
            assert_eq!(cache.take_plans().len(), 1);
        }
    }
}
