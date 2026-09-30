//! Reusable append-only evidence records and prompt-prefix cost comparison.
//! This module knows nothing about files, tools, providers, or conversation roles.

use std::time::Duration;

use serde::Serialize;

/// Immutable observations. A new version is appended, never written over the old one.
#[derive(Debug)]
pub struct RevisionJournal<T> {
    records: Vec<T>,
}

impl<T> Default for RevisionJournal<T> {
    fn default() -> Self {
        Self {
            records: Vec::new(),
        }
    }
}

impl<T> RevisionJournal<T> {
    pub fn append(&mut self, observation: T) -> usize {
        let index = self.records.len();
        self.records.push(observation);
        index
    }

    pub fn records(&self) -> &[T] {
        &self.records
    }
}

#[derive(Debug, Clone, Copy)]
pub struct InputRates {
    /// USD per million tokens, supplied by live model metadata.
    pub uncached: f64,
    pub cached: f64,
    pub cache_write: Option<f64>,
}

#[derive(Debug, Clone, Copy)]
pub struct UpdateEstimate {
    pub context_tokens: u64,
    /// All context from the first changed observation through the current tail.
    pub suffix_tokens: u64,
    pub obsolete_tokens: u64,
    pub correction_tokens: u64,
    pub reference_tokens: u64,
    pub observation_age: Option<Duration>,
    pub cache_idle_age: Duration,
    /// None means retention is unknown; age is reported without inventing a TTL.
    pub cache_ttl: Option<Duration>,
    /// Measured cache coverage is an estimate, not a promise of a cache hit.
    pub cache_hit_fraction: Option<f64>,
    pub expected_reuses: u32,
    pub rates: Option<InputRates>,
    /// Retirement may cross a model input-price tier.
    pub retire_rates: Option<InputRates>,
    /// Historical evidence needed for execution/provenance must stay intact.
    pub may_retire_obsolete_body: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateStrategy {
    Append,
    RetireObsoleteBody,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdatePlan {
    pub strategy: UpdateStrategy,
    pub cheaper_strategy: Option<UpdateStrategy>,
    pub append_cost_usd: Option<f64>,
    pub retire_cost_usd: Option<f64>,
    pub observation_age_seconds: Option<f64>,
    pub cache_idle_seconds: f64,
    pub suffix_tokens: u64,
    pub append_uncached_tokens: Option<f64>,
    pub retire_uncached_tokens: Option<f64>,
    pub expected_reuses: u32,
    pub reason: &'static str,
}

pub fn plan_update(input: UpdateEstimate) -> UpdatePlan {
    let mut plan = UpdatePlan {
        strategy: UpdateStrategy::Append,
        cheaper_strategy: None,
        append_cost_usd: None,
        retire_cost_usd: None,
        observation_age_seconds: input.observation_age.map(|age| age.as_secs_f64()),
        cache_idle_seconds: input.cache_idle_age.as_secs_f64(),
        suffix_tokens: input.suffix_tokens,
        append_uncached_tokens: None,
        retire_uncached_tokens: None,
        expected_reuses: input.expected_reuses,
        reason: "missing_pricing_or_cache_measurement",
    };
    let (Some(rates), Some(mut hit)) = (input.rates, input.cache_hit_fraction) else {
        return plan;
    };
    if ![
        rates.uncached,
        rates.cached,
        rates.cache_write.unwrap_or(rates.uncached),
        hit,
    ]
    .iter()
    .all(|value| value.is_finite() && *value >= 0.0)
        || hit > 1.0
        || input.suffix_tokens > input.context_tokens
        || input.obsolete_tokens > input.suffix_tokens
    {
        plan.reason = "invalid_cost_inputs";
        return plan;
    }
    if input
        .cache_ttl
        .is_some_and(|ttl| input.cache_idle_age >= ttl)
    {
        hit = 0.0;
    }
    let total = input.context_tokens as f64;
    let suffix = input.suffix_tokens as f64;
    let prefix = total - suffix;
    let retired = input.obsolete_tokens as f64;
    let reference = input.reference_tokens as f64;
    let correction = input.correction_tokens as f64;
    let retire_rates = input.retire_rates.unwrap_or(rates);
    if ![
        retire_rates.uncached,
        retire_rates.cached,
        retire_rates.cache_write.unwrap_or(retire_rates.uncached),
    ]
    .iter()
    .all(|value| value.is_finite() && *value >= 0.0)
    {
        plan.reason = "invalid_cost_inputs";
        return plan;
    }
    let cold_rate = rates.cache_write.unwrap_or(rates.uncached);
    let retire_cold_rate = retire_rates.cache_write.unwrap_or(retire_rates.uncached);
    // Prompt caches match a prefix, not randomly distributed individual tokens.
    let cached_prefix = total * hit;
    let retained_cached_prefix = prefix.min(cached_prefix);
    let append_cold = total - cached_prefix + correction;
    let retire_cold = prefix - retained_cached_prefix + suffix - retired + reference + correction;
    // Future calls can cache either resulting prompt. Retiring a large stale
    // body saves replay tokens, but first pays for reprocessing its whole suffix.
    let replay_rate = hit * rates.cached + (1.0 - hit) * cold_rate;
    let future = f64::from(input.expected_reuses);
    let append_cost = (append_cold * cold_rate
        + total * hit * rates.cached
        + future * (total + correction) * replay_rate)
        / 1_000_000.0;
    let retire_replay_rate = hit * retire_rates.cached + (1.0 - hit) * retire_cold_rate;
    let retire_cost = (retire_cold * retire_cold_rate
        + retained_cached_prefix * retire_rates.cached
        + future * (total - retired + reference + correction) * retire_replay_rate)
        / 1_000_000.0;
    plan.append_cost_usd = Some(append_cost);
    plan.retire_cost_usd = Some(retire_cost);
    plan.append_uncached_tokens = Some(append_cold);
    plan.retire_uncached_tokens = Some(retire_cold);
    let cheaper = if retire_cost < append_cost {
        UpdateStrategy::RetireObsoleteBody
    } else {
        UpdateStrategy::Append
    };
    plan.cheaper_strategy = Some(cheaper);
    if !input.may_retire_obsolete_body {
        plan.reason = "historical_evidence_required";
    } else {
        plan.strategy = cheaper;
        plan.reason = "estimated_total_input_cost";
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn economics_account_for_suffix_expiry_future_replays_and_evidence_constraints() {
        let base = UpdateEstimate {
            context_tokens: 100_000,
            suffix_tokens: 90_000,
            obsolete_tokens: 10_000,
            correction_tokens: 200,
            reference_tokens: 50,
            observation_age: Some(Duration::from_secs(600)),
            cache_idle_age: Duration::from_secs(30),
            cache_ttl: Some(Duration::from_secs(300)),
            cache_hit_fraction: Some(1.0),
            expected_reuses: 1,
            rates: Some(InputRates {
                uncached: 10.0,
                cached: 1.0,
                cache_write: Some(12.5),
            }),
            retire_rates: None,
            may_retire_obsolete_body: true,
        };
        let append = plan_update(base);
        assert_eq!(append.strategy, UpdateStrategy::Append);
        assert_eq!(append.append_uncached_tokens, Some(200.0));
        assert_eq!(append.retire_uncached_tokens, Some(80_250.0));
        assert_eq!(append.observation_age_seconds, Some(600.0));
        let partial_hit = plan_update(UpdateEstimate {
            cache_hit_fraction: Some(0.5),
            ..base
        });
        assert_eq!(partial_hit.append_uncached_tokens, Some(50_200.0));
        assert_eq!(partial_hit.retire_uncached_tokens, Some(80_250.0));
        assert_eq!(
            plan_update(UpdateEstimate {
                cache_idle_age: Duration::from_secs(301),
                ..base
            })
            .strategy,
            UpdateStrategy::RetireObsoleteBody
        );
        assert_eq!(
            plan_update(UpdateEstimate {
                expected_reuses: 100,
                ..base
            })
            .strategy,
            UpdateStrategy::RetireObsoleteBody
        );
        assert_eq!(
            plan_update(UpdateEstimate {
                cache_hit_fraction: None,
                ..base
            })
            .strategy,
            UpdateStrategy::Append
        );
        assert_eq!(
            plan_update(UpdateEstimate {
                expected_reuses: 100,
                may_retire_obsolete_body: false,
                ..base
            })
            .strategy,
            UpdateStrategy::Append
        );
    }
}
