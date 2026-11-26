//! Enforced cost budget (issue #1400).
//!
//! On top of the normalized resource/cost ledger ([`crate::resource_ledger`],
//! issue #1371) this module turns a *user-declared* rate table into a hard cap:
//! the scheduler stops dispatching **new** work once the cost accrued from the
//! rules that already executed reaches the declared `amount`.
//!
//! It is deliberately **not** authoritative billing: there is no price catalog
//! and no per-vendor API. The operator declares the rates, and they are applied
//! to the same measured CPU-seconds and GB-seconds the report surfaces:
//!
//! ```text
//! accrued = cpu_second_rate * sum(rule.cpu_seconds)
//!         + gb_second_rate   * sum(max_rss_bytes / 1e9 * wall_time_secs)
//! ```
//!
//! The sums run over the rules that *actually executed* in the run — the same
//! predicate the rendered ledger uses — so accounting and its cap can never
//! disagree about which rules count. Invariant: **unknown is never zero.** When
//! a configured rate depends on a metric an executed rule did not report, the
//! accrued total is `None` (never a partial sum treated as complete) and the
//! offending rule names are listed for the operator.

use crate::config::CostBudget;
use crate::error::{OxoFlowError, Result};
use crate::resource_ledger::{LedgerRule, rule_actually_executed};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Default `on_unknown` policy when the budget does not set one: fail closed.
pub const DEFAULT_ON_UNKNOWN: &str = "refuse";

/// The result of accruing a [`CostBudget`] over a set of ledger rules.
///
/// `accrued` is `None` when the total cannot be known (a configured rate's
/// input metric is missing for some executed rule); [`Self::unknown_rules`]
/// names them. `cpu_cost` / `gb_cost` are `None` when the corresponding rate is
/// not configured. `currency_mismatch` lists executed rules whose reported
/// `cost.currency` differs from the budget currency — that is **always**
/// fail-closed regardless of [`Self::on_unknown`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CostAccrual {
    /// Currency label of the budget (equality-only comparison).
    pub currency: String,
    /// The declared cap, in `currency`.
    pub limit: f64,
    /// Total CPU-seconds accrued over executed rules (`0.0` when no rule
    /// reported any and none is missing).
    pub cpu_seconds: f64,
    /// Total GB-seconds accrued over executed rules.
    pub gb_seconds: f64,
    /// `cpu_second_rate * cpu_seconds`, or `None` when no CPU rate configured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_cost: Option<f64>,
    /// `gb_second_rate * gb_seconds`, or `None` when no GB rate configured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gb_cost: Option<f64>,
    /// Total accrued cost, or `None` when it cannot be known (never a partial
    /// sum presented as complete).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accrued: Option<f64>,
    /// Executed rules whose configured-rate input metric was missing.
    pub unknown_rules: Vec<String>,
    /// Executed rules whose reported cost currency differs from the budget.
    pub currency_mismatch: Vec<String>,
    /// Stable human string naming the rates the accrual used.
    pub method: String,
    /// The `on_unknown` policy in force (`"refuse"` or `"report"`).
    pub on_unknown: String,
}

impl CostAccrual {
    /// Whether accrual is unknown and the policy is to fail closed.
    #[must_use]
    pub fn refuses_on_unknown(&self) -> bool {
        self.accrued.is_none() && self.on_unknown == DEFAULT_ON_UNKNOWN
    }

    /// Whether the accrued cost has reached or passed the declared limit.
    #[must_use]
    pub fn exceeded(&self) -> bool {
        matches!(self.accrued, Some(v) if v >= self.limit)
    }

    /// Whether enforcement must stop new dispatch right now: the cap is
    /// reached, accrual is unknown under a `refuse` policy, or a currency
    /// mismatch was reported (always fail-closed).
    #[must_use]
    pub fn must_stop(&self) -> bool {
        self.exceeded() || self.refuses_on_unknown() || !self.currency_mismatch.is_empty()
    }
}

/// Accrue a [`CostBudget`] over `rules` (pure; no I/O).
///
/// Only rules that actually executed contribute. A rate that is configured but
/// whose input metric is missing on an executed rule makes `accrued = None` and
/// records that rule in `unknown_rules` — the partial sum is never presented as
/// a complete total.
#[must_use]
pub fn accrue_cost(rules: &[LedgerRule], budget: &CostBudget) -> CostAccrual {
    let on_unknown = budget
        .on_unknown
        .clone()
        .unwrap_or_else(|| DEFAULT_ON_UNKNOWN.to_string());

    let mut cpu_seconds = 0.0_f64;
    let mut gb_seconds = 0.0_f64;
    let mut cpu_unknown = false;
    let mut gb_unknown = false;
    let mut unknown_rules: BTreeSet<String> = BTreeSet::new();
    let mut currency_mismatch: BTreeSet<String> = BTreeSet::new();

    let cpu_rate = budget.cpu_second_rate;
    let gb_rate = budget.gb_second_rate;

    for rule in rules.iter().filter(|r| rule_actually_executed(r)) {
        // A rule whose reported cost is in a different currency can never be
        // folded into this budget — fail closed regardless of `on_unknown`.
        if let Some(cost) = &rule.cost
            && cost.currency != budget.currency
        {
            currency_mismatch.insert(rule.rule.clone());
        }

        if cpu_rate.is_some() {
            match rule.cpu_seconds {
                Some(v) => cpu_seconds += v,
                None => {
                    cpu_unknown = true;
                    unknown_rules.insert(rule.rule.clone());
                }
            }
        }

        if gb_rate.is_some() {
            match (rule.max_rss_bytes, rule.wall_time_secs) {
                (Some(rss), Some(wall)) => gb_seconds += (rss as f64 / 1e9) * wall,
                _ => {
                    gb_unknown = true;
                    unknown_rules.insert(rule.rule.clone());
                }
            }
        }
    }

    let cpu_cost = cpu_rate.map(|r| r * cpu_seconds);
    let gb_cost = gb_rate.map(|r| r * gb_seconds);
    let accrued = if cpu_unknown || gb_unknown {
        None
    } else {
        Some(cpu_cost.unwrap_or(0.0) + gb_cost.unwrap_or(0.0))
    };

    CostAccrual {
        currency: budget.currency.clone(),
        limit: budget.amount,
        cpu_seconds,
        gb_seconds,
        cpu_cost,
        gb_cost,
        accrued,
        unknown_rules: unknown_rules.into_iter().collect(),
        currency_mismatch: currency_mismatch.into_iter().collect(),
        method: accrual_method(cpu_rate.is_some(), gb_rate.is_some()),
        on_unknown,
    }
}

/// Stable human string naming the rates the accrual uses. Kept independent of
/// the numbers so a report row says *how* the figure was derived.
fn accrual_method(has_cpu: bool, has_gb: bool) -> String {
    match (has_cpu, has_gb) {
        (true, true) => {
            "cpu_second_rate * cpu_seconds + gb_second_rate * (max_rss_bytes/1e9 * wall_time_secs)"
                .to_string()
        }
        (true, false) => "cpu_second_rate * cpu_seconds".to_string(),
        (false, true) => "gb_second_rate * (max_rss_bytes/1e9 * wall_time_secs)".to_string(),
        (false, false) => "no configured rate".to_string(),
    }
}

/// Describe a declared [`CostBudget`] at **plan time** (`dry-run` / `validate`),
/// where no rule has executed yet and the run-time accrual is therefore
/// *unestimable*.
///
/// Returns the JSON object the plan-time surfaces embed: the declared cap and
/// rates plus the canonical `accrual_method` string (so the human and JSON
/// surfaces name the same accrual the dispatcher will compute at run time).
/// `accrued` is always `null` and `estimate_available` is `false` — a plan-time
/// cost is **never** a fabricated `0`; only a real run produces a number.
#[must_use]
pub fn plan_time_budget_json(budget: &CostBudget) -> serde_json::Value {
    // `accrue_cost` over the (empty) plan-time rule set gives the canonical
    // method string — only that string is used; its `accrued` is discarded so
    // no plan-time total is ever derived from the empty set.
    let method = accrue_cost(&[], budget).method;
    let on_unknown = budget
        .on_unknown
        .clone()
        .unwrap_or_else(|| DEFAULT_ON_UNKNOWN.to_string());
    let mut obj = serde_json::Map::new();
    obj.insert("amount".to_string(), serde_json::json!(budget.amount));
    obj.insert("currency".to_string(), serde_json::json!(budget.currency));
    if let Some(r) = budget.cpu_second_rate {
        obj.insert("cpu_second_rate".to_string(), serde_json::json!(r));
    }
    if let Some(r) = budget.gb_second_rate {
        obj.insert("gb_second_rate".to_string(), serde_json::json!(r));
    }
    obj.insert("on_unknown".to_string(), serde_json::json!(on_unknown));
    obj.insert("method".to_string(), serde_json::json!(method));
    obj.insert("accrued".to_string(), serde_json::Value::Null);
    obj.insert("estimate_available".to_string(), serde_json::json!(false));
    obj.insert(
        "note".to_string(),
        serde_json::json!("plan-time cost is unestimable: no rule has executed yet"),
    );
    serde_json::Value::Object(obj)
}

/// Validate a declared `[resource_budget].max_cost` table at load/preflight
/// time. A zero or unreadable cap could never be meaningful, and a budget with
/// no rate would accrue a constant `0` that never trips — both are rejected.
///
/// # Errors
///
/// Returns [`OxoFlowError::Config`] when `amount` is not finite/positive,
/// `currency` is empty, no rate is present, or a present rate is not a finite
/// non-negative number.
pub fn validate_cost_budget(budget: &CostBudget) -> Result<()> {
    if !budget.amount.is_finite() || budget.amount <= 0.0 {
        return Err(OxoFlowError::Config {
            message: format!(
                "[resource_budget] max_cost amount = {} must be a finite value greater than 0",
                budget.amount
            ),
        });
    }
    if budget.currency.trim().is_empty() {
        return Err(OxoFlowError::Config {
            message: "[resource_budget] max_cost currency must not be empty".to_string(),
        });
    }
    for (name, rate) in [
        ("cpu_second_rate", budget.cpu_second_rate),
        ("gb_second_rate", budget.gb_second_rate),
    ] {
        if let Some(r) = rate
            && (!r.is_finite() || r < 0.0)
        {
            return Err(OxoFlowError::Config {
                message: format!(
                    "[resource_budget] max_cost {name} = {r} must be a finite value >= 0"
                ),
            });
        }
    }
    // At least one declared rate must be strictly positive. A lone `0` (or two
    // zero rates) accrues a constant 0 forever — the run would appear budgeted
    // yet never stop, so a typo like `gb_second_rate = 0` silently disables
    // enforcement. A `0` alongside a positive rate is still fine (it just
    // leaves that metric unpriced), so the check is "any rate > 0", not "no
    // rate may be 0".
    let any_positive = budget.cpu_second_rate.is_some_and(|r| r > 0.0)
        || budget.gb_second_rate.is_some_and(|r| r > 0.0);
    if !any_positive {
        return Err(OxoFlowError::Config {
            message: "[resource_budget] max_cost needs at least one of cpu_second_rate or \
                      gb_second_rate greater than 0 — with only zero rates the accrual is \
                      always 0 and the cap can never trip"
                .to_string(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource_ledger::{LedgerRule, ResourceSource};

    fn rule(name: &str, cpu: Option<f64>, rss: Option<u64>, wall: Option<f64>) -> LedgerRule {
        LedgerRule {
            rule: name.to_string(),
            status: "completed".to_string(),
            wall_time_secs: wall,
            cpu_seconds: cpu,
            user_seconds: None,
            system_seconds: None,
            max_rss_bytes: rss,
            peak_memory_bytes: None,
            io_read_bytes: None,
            io_write_bytes: None,
            gpu_seconds: None,
            scheduler_wait_secs: None,
            nodes: None,
            cores: None,
            cost: None,
            source: ResourceSource::unknown("test"),
            recorded_as: None,
        }
    }

    fn budget(cpu: Option<f64>, gb: Option<f64>) -> CostBudget {
        CostBudget {
            amount: 10.0,
            currency: "USD".to_string(),
            cpu_second_rate: cpu,
            gb_second_rate: gb,
            on_unknown: None,
        }
    }

    #[test]
    fn accrual_arithmetic_both_rates() {
        // rule a: cpu 100, rss 2e9 (2 GB), wall 50 -> gb-s = 2*50 = 100
        // rule b: cpu 300, rss 1e9 (1 GB), wall 10 -> gb-s = 1*10 = 10
        let rules = vec![
            rule("a", Some(100.0), Some(2_000_000_000), Some(50.0)),
            rule("b", Some(300.0), Some(1_000_000_000), Some(10.0)),
        ];
        let acc = accrue_cost(&rules, &budget(Some(0.001), Some(0.01)));
        assert_eq!(acc.cpu_seconds, 400.0);
        assert_eq!(acc.gb_seconds, 110.0);
        assert_eq!(acc.cpu_cost, Some(0.4));
        assert_eq!(acc.gb_cost, Some(1.1));
        assert_eq!(acc.accrued, Some(1.5));
        assert!(acc.unknown_rules.is_empty());
        assert!(!acc.must_stop());
    }

    #[test]
    fn missing_cpu_makes_accrual_unknown_and_names_rule() {
        let rules = vec![
            rule("ok", Some(100.0), None, None),
            rule("noc", None, None, None),
        ];
        let acc = accrue_cost(&rules, &budget(Some(0.001), None));
        assert_eq!(acc.accrued, None);
        assert_eq!(acc.unknown_rules, vec!["noc".to_string()]);
        assert!(acc.refuses_on_unknown());
        assert!(acc.must_stop());
    }

    #[test]
    fn missing_wall_time_makes_gb_unknown() {
        let rules = vec![rule("a", None, Some(1_000_000_000), None)];
        let acc = accrue_cost(&rules, &budget(None, Some(0.01)));
        assert_eq!(acc.accrued, None);
        assert_eq!(acc.unknown_rules, vec!["a".to_string()]);
    }

    #[test]
    fn report_policy_does_not_refuse() {
        let mut b = budget(Some(0.001), None);
        b.on_unknown = Some("report".to_string());
        let rules = vec![rule("noc", None, None, None)];
        let acc = accrue_cost(&rules, &b);
        assert_eq!(acc.accrued, None);
        assert!(!acc.refuses_on_unknown());
        // Unknown numbers under "report": not exceeded, but a currency mismatch
        // would still stop. No mismatch here, so the gate lets work continue.
        assert!(!acc.must_stop());
        assert_eq!(acc.on_unknown, "report");
    }

    #[test]
    fn currency_mismatch_always_stops() {
        let mut r = rule("a", Some(1.0), None, None);
        r.cost = Some(crate::resource_ledger::Cost {
            amount: 1.0,
            currency: "EUR".to_string(),
            method: "test".to_string(),
            source: None,
        });
        let acc = accrue_cost(&[r], &budget(Some(0.001), None));
        assert_eq!(acc.currency_mismatch, vec!["a".to_string()]);
        assert!(acc.must_stop());
        // Even under "report", a mismatch fails closed.
        let mut b = budget(Some(0.001), None);
        b.on_unknown = Some("report".to_string());
        let acc = accrue_cost(&[rule_with_cost("a", "EUR")], &b);
        assert!(acc.must_stop());
    }

    fn rule_with_cost(name: &str, currency: &str) -> LedgerRule {
        let mut r = rule(name, Some(1.0), None, None);
        r.cost = Some(crate::resource_ledger::Cost {
            amount: 1.0,
            currency: currency.to_string(),
            method: "test".to_string(),
            source: None,
        });
        r
    }

    #[test]
    fn cap_reached_is_exceeded() {
        let rules = vec![rule("a", Some(10_000.0), None, None)];
        let acc = accrue_cost(&rules, &budget(Some(0.001), None));
        assert_eq!(acc.accrued, Some(10.0));
        assert!(acc.exceeded());
        assert!(acc.must_stop());
    }

    #[test]
    fn empty_rules_accrue_zero() {
        let acc = accrue_cost(&[], &budget(Some(0.001), Some(0.01)));
        assert_eq!(acc.accrued, Some(0.0));
        assert_eq!(acc.cpu_seconds, 0.0);
        assert_eq!(acc.gb_seconds, 0.0);
        assert!(!acc.must_stop());
    }

    #[test]
    fn skipped_and_synthetic_rules_do_not_count() {
        let mut skipped = rule("sk", Some(1000.0), None, None);
        skipped.status = "skipped".to_string();
        let mut synth = rule("sy", Some(1000.0), None, None);
        synth.recorded_as = Some("outputs up-to-date".to_string());
        let acc = accrue_cost(&[skipped, synth], &budget(Some(0.001), None));
        assert_eq!(acc.accrued, Some(0.0));
        assert!(acc.unknown_rules.is_empty());
    }

    #[test]
    fn method_names_the_rates() {
        assert_eq!(
            accrue_cost(&[], &budget(Some(0.001), Some(0.01))).method,
            "cpu_second_rate * cpu_seconds + gb_second_rate * (max_rss_bytes/1e9 * wall_time_secs)"
        );
        assert_eq!(
            accrue_cost(&[], &budget(Some(0.001), None)).method,
            "cpu_second_rate * cpu_seconds"
        );
    }

    #[test]
    fn plan_time_budget_json_is_unestimable() {
        let obj = plan_time_budget_json(&budget(Some(0.001), Some(0.01)));
        assert_eq!(obj["amount"], serde_json::json!(10.0));
        assert_eq!(obj["currency"], serde_json::json!("USD"));
        assert_eq!(obj["on_unknown"], serde_json::json!("refuse"));
        assert_eq!(obj["estimate_available"], serde_json::json!(false));
        assert!(obj["accrued"].is_null());
        assert_eq!(
            obj["method"],
            serde_json::json!(
                "cpu_second_rate * cpu_seconds + gb_second_rate * (max_rss_bytes/1e9 * wall_time_secs)"
            )
        );
        // A CPU-only budget names only that rate and omits the absent one.
        let only_cpu = plan_time_budget_json(&budget(Some(0.001), None));
        assert_eq!(
            only_cpu["method"],
            serde_json::json!("cpu_second_rate * cpu_seconds")
        );
        assert!(only_cpu.get("gb_second_rate").is_none());
    }

    #[test]
    fn validation_rejects_bad_budgets() {
        assert!(validate_cost_budget(&budget(Some(0.001), None)).is_ok());
        // amount <= 0 / non-finite
        let mut b = budget(Some(0.001), None);
        b.amount = 0.0;
        assert!(validate_cost_budget(&b).is_err());
        let mut b = budget(Some(0.001), None);
        b.amount = f64::NAN;
        assert!(validate_cost_budget(&b).is_err());
        // empty currency
        let mut b = budget(Some(0.001), None);
        b.currency = "  ".to_string();
        assert!(validate_cost_budget(&b).is_err());
        // no rate
        assert!(validate_cost_budget(&budget(None, None)).is_err());
        // negative / non-finite rate
        assert!(validate_cost_budget(&budget(Some(-1.0), None)).is_err());
        assert!(validate_cost_budget(&budget(None, Some(f64::INFINITY))).is_err());
        // zero rate is allowed alongside a positive one (a free dimension)…
        assert!(validate_cost_budget(&budget(Some(0.0), Some(0.5))).is_ok());
        // …but a budget whose only rates are zero can never trip, so it is rejected.
        assert!(validate_cost_budget(&budget(Some(0.0), None)).is_err());
        assert!(validate_cost_budget(&budget(Some(0.0), Some(0.0))).is_err());
    }
}
