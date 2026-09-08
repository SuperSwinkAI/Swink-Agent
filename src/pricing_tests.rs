//! Tests for `pricing`.
#![cfg(test)]

use super::*;

fn usage(input: u64, output: u64) -> Usage {
    Usage {
        input,
        output,
        ..Usage::default()
    }
}

#[test]
fn model_rates_prices_each_category() {
    let rates = ModelRates {
        input_per_million: 1.0,
        output_per_million: 2.0,
        cache_read_per_million: 0.5,
        cache_write_per_million: 4.0,
    };
    let usage = Usage {
        input: 1_000_000,
        output: 1_000_000,
        cache_read: 1_000_000,
        cache_write: 1_000_000,
        ..Usage::default()
    };
    let cost = rates.cost_for(&usage);
    assert!((cost.input - 1.0).abs() < 1e-9);
    assert!((cost.output - 2.0).abs() < 1e-9);
    assert!((cost.cache_read - 0.5).abs() < 1e-9);
    assert!((cost.cache_write - 4.0).abs() < 1e-9);
    assert!((cost.total - 7.5).abs() < 1e-9);
}

#[test]
fn default_rates_price_to_zero() {
    assert!(
        ModelRates::default()
            .cost_for(&usage(1_000_000, 1_000_000))
            .is_zero()
    );
}

#[test]
fn pricing_table_declines_unknown_model() {
    let table = PricingTable::new().with_model("known", ModelRates::default());
    assert!(table.calculate("unknown", &usage(1, 1)).is_none());
}

#[test]
fn pricing_table_prices_declared_model() {
    let table = PricingTable::new().with_model(
        "local",
        ModelRates {
            input_per_million: 3.0,
            ..ModelRates::default()
        },
    );
    let cost = table.calculate("local", &usage(2_000_000, 0)).unwrap();
    assert!((cost.total - 6.0).abs() < 1e-9);
}

#[test]
fn pricing_table_with_model_replaces_previous_entry() {
    let table = PricingTable::new()
        .with_model(
            "m",
            ModelRates {
                input_per_million: 1.0,
                ..ModelRates::default()
            },
        )
        .with_model(
            "m",
            ModelRates {
                input_per_million: 9.0,
                ..ModelRates::default()
            },
        );
    assert_eq!(table.len(), 1);
    let cost = table.calculate("m", &usage(1_000_000, 0)).unwrap();
    assert!((cost.total - 9.0).abs() < 1e-9);
}

#[test]
fn empty_table_is_empty() {
    let table = PricingTable::new();
    assert!(table.is_empty());
    assert_eq!(table.len(), 0);
    assert_eq!(table.iter().count(), 0);
}

#[test]
fn closures_implement_cost_calculator() {
    let calculator = |model_id: &str, _usage: &Usage| -> Option<Cost> {
        (model_id == "flat").then(|| Cost {
            total: 0.25,
            ..Cost::default()
        })
    };
    assert!((calculator.calculate("flat", &usage(1, 1)).unwrap().total - 0.25).abs() < 1e-9);
    assert!(calculator.calculate("other", &usage(1, 1)).is_none());
}

#[test]
fn pricing_table_deserializes_from_toml_table() {
    let toml = r#"
            ["my-local-llama"]
            input_per_million = 0.10
            output_per_million = 0.40

            ["partial-rates"]
            input_per_million = 1.0
        "#;
    let table: PricingTable = toml::from_str(toml).expect("table should parse");
    assert_eq!(table.len(), 2);

    let cost = table
        .calculate("my-local-llama", &usage(1_000_000, 1_000_000))
        .unwrap();
    assert!((cost.total - 0.50).abs() < 1e-9);

    // Unspecified categories are free, not "inherit from catalog".
    let partial = table.get("partial-rates").unwrap();
    assert!((partial.output_per_million).abs() < 1e-9);
}

#[test]
fn pricing_table_collects_from_iterator() {
    let table: PricingTable = vec![("a".to_string(), ModelRates::default())]
        .into_iter()
        .collect();
    assert_eq!(table.len(), 1);
    assert!(table.get("a").is_some());
}
