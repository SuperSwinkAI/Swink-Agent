//! Operator-declared model pricing.
//!
//! The agent loop prices every assistant message that its adapter left
//! unpriced, using the compiled model catalog (see
//! [`price_assistant_message`](crate::price_assistant_message)). The catalog
//! only knows about models shipped with the crate, so local endpoints, private
//! deployments, and negotiated per-tier rates all price at zero.
//!
//! This module is the escape hatch: a host binary can declare its own rates and
//! hand them to the loop via
//! [`AgentOptions::with_cost_calculator`](crate::AgentOptions::with_cost_calculator)
//! or [`AgentOptions::with_pricing_table`](crate::AgentOptions::with_pricing_table).
//!
//! # Precedence
//!
//! For each assistant message the loop resolves cost in this order:
//!
//! 1. **The adapter's own cost**, when non-zero. Only the proxy adapter reports
//!    real provider-billed amounts, and those always win.
//! 2. **The operator-declared [`CostCalculator`]**, when one is configured and
//!    it returns a non-zero [`Cost`] for the model.
//! 3. **The compiled model catalog**.
//!
//! A message stays at zero only when all three decline.
//!
//! # Example
//!
//! ```rust
//! use swink_agent::{ModelRates, PricingTable, CostCalculator, Usage};
//!
//! let table = PricingTable::new().with_model(
//!     "my-local-llama",
//!     ModelRates::default()
//!         .with_input_per_million(0.10)
//!         .with_output_per_million(0.40),
//! );
//!
//! let usage = Usage::default().with_input(1_000_000).with_output(1_000_000);
//! let cost = table.calculate("my-local-llama", &usage).unwrap();
//! assert!((cost.total - 0.50).abs() < 1e-9);
//! ```

use std::collections::HashMap;

use serde::Deserialize;

use crate::types::{Cost, Usage};

/// Per-model rates, expressed in USD per million tokens.
///
/// Deserializes from a TOML table so operators can declare rates in a config
/// file:
///
/// ```toml
/// [pricing."my-local-llama"]
/// input_per_million = 0.10
/// output_per_million = 0.40
/// ```
///
/// Unset categories default to `0.0`, which means "this category is free",
/// not "fall back to the catalog". Fallback happens per-model, not per-field:
/// see the module-level precedence rules.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ModelRates {
    /// USD per million input (prompt) tokens.
    pub input_per_million: f64,
    /// USD per million output (completion) tokens.
    pub output_per_million: f64,
    /// USD per million tokens read from a provider-side prompt cache.
    pub cache_read_per_million: f64,
    /// USD per million tokens written to a provider-side prompt cache.
    pub cache_write_per_million: f64,
}

impl ModelRates {
    /// Set the USD-per-million-input-token rate.
    #[must_use]
    pub const fn with_input_per_million(mut self, rate: f64) -> Self {
        self.input_per_million = rate;
        self
    }

    /// Set the USD-per-million-output-token rate.
    #[must_use]
    pub const fn with_output_per_million(mut self, rate: f64) -> Self {
        self.output_per_million = rate;
        self
    }

    /// Set the USD-per-million-cache-read-token rate.
    #[must_use]
    pub const fn with_cache_read_per_million(mut self, rate: f64) -> Self {
        self.cache_read_per_million = rate;
        self
    }

    /// Set the USD-per-million-cache-write-token rate.
    #[must_use]
    pub const fn with_cache_write_per_million(mut self, rate: f64) -> Self {
        self.cache_write_per_million = rate;
        self
    }

    /// Price a [`Usage`] with these rates.
    #[must_use]
    pub fn cost_for(&self, usage: &Usage) -> Cost {
        #[allow(clippy::cast_precision_loss)] // token counts fit comfortably in f64
        let per_m = |tokens: u64, rate: f64| -> f64 { tokens as f64 * rate / 1_000_000.0 };

        let input = per_m(usage.input, self.input_per_million);
        let output = per_m(usage.output, self.output_per_million);
        let cache_read = per_m(usage.cache_read, self.cache_read_per_million);
        let cache_write = per_m(usage.cache_write, self.cache_write_per_million);

        Cost {
            input,
            output,
            cache_read,
            cache_write,
            total: input + output + cache_read + cache_write,
            ..Cost::default()
        }
    }
}

/// Resolves a [`Cost`] for a model's token [`Usage`].
///
/// Implemented by [`PricingTable`], and blanket-implemented for any
/// `Fn(&str, &Usage) -> Option<Cost>` closure, so hosts can supply either a
/// declarative rate table or arbitrary logic (tiered rates, per-tenant
/// markups, a rates service).
///
/// Return `None` — or a zero [`Cost`] — to decline pricing and let the loop
/// fall back to the compiled model catalog.
pub trait CostCalculator: Send + Sync {
    /// Price `usage` for `model_id`, or return `None` to decline.
    fn calculate(&self, model_id: &str, usage: &Usage) -> Option<Cost>;
}

impl<F> CostCalculator for F
where
    F: Fn(&str, &Usage) -> Option<Cost> + Send + Sync,
{
    fn calculate(&self, model_id: &str, usage: &Usage) -> Option<Cost> {
        self(model_id, usage)
    }
}

/// An operator-declared table of per-model rates, keyed by model ID.
///
/// Deserializes transparently from a TOML table of tables, so a `[pricing]`
/// config section maps straight onto it. See [`ModelRates`] for the per-model
/// shape and the [module docs](self) for how the table interacts with catalog
/// pricing.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(transparent)]
pub struct PricingTable {
    rates: HashMap<String, ModelRates>,
}

impl PricingTable {
    /// Create an empty table.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Declare rates for a model ID, replacing any previous entry.
    #[must_use]
    pub fn with_model(mut self, model_id: impl Into<String>, rates: ModelRates) -> Self {
        self.rates.insert(model_id.into(), rates);
        self
    }

    /// Look up the declared rates for a model ID.
    #[must_use]
    pub fn get(&self, model_id: &str) -> Option<&ModelRates> {
        self.rates.get(model_id)
    }

    /// Number of models with declared rates.
    #[must_use]
    pub fn len(&self) -> usize {
        self.rates.len()
    }

    /// Whether any rates are declared.
    ///
    /// An empty table declines every model, so the loop falls back to the
    /// catalog for all of them.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rates.is_empty()
    }

    /// Iterate over the declared `(model_id, rates)` pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &ModelRates)> {
        self.rates.iter().map(|(id, rates)| (id.as_str(), rates))
    }
}

impl CostCalculator for PricingTable {
    fn calculate(&self, model_id: &str, usage: &Usage) -> Option<Cost> {
        self.rates.get(model_id).map(|rates| rates.cost_for(usage))
    }
}

impl FromIterator<(String, ModelRates)> for PricingTable {
    fn from_iter<I: IntoIterator<Item = (String, ModelRates)>>(iter: I) -> Self {
        Self {
            rates: iter.into_iter().collect(),
        }
    }
}

#[cfg(test)]
#[path = "pricing_tests.rs"]
mod tests;
