//! Value helpers used to build a chart from a report result.

use ferris_core::Value;

use super::types::ChartAggregate;

/// Label used to group rows: strings as-is, `null` as empty, others as JSON.
pub(super) fn value_label(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_json().to_string(),
    }
}

/// Extract a number from a cell, accepting numeric strings.
pub(super) fn row_number(value: Option<&Value>) -> Option<f64> {
    match value {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) => s.parse::<f64>().ok(),
        _ => None,
    }
}

pub(super) fn aggregate_values(values: Vec<Option<f64>>, aggregate: ChartAggregate) -> f64 {
    match aggregate {
        ChartAggregate::Count => values.len() as f64,
        ChartAggregate::Sum => values.iter().flatten().sum(),
        ChartAggregate::Average => {
            let numbers: Vec<f64> = values.iter().flatten().copied().collect();
            if numbers.is_empty() {
                0.0
            } else {
                numbers.iter().sum::<f64>() / numbers.len() as f64
            }
        }
        ChartAggregate::Min => values
            .iter()
            .flatten()
            .copied()
            .reduce(f64::min)
            .unwrap_or(0.0),
        ChartAggregate::Max => values
            .iter()
            .flatten()
            .copied()
            .reduce(f64::max)
            .unwrap_or(0.0),
    }
}
