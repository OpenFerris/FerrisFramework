//! Numeric accumulation for `docstore::aggregate`.

use crate::host::DocAggregateSpec;
use ferris_core::Value;

/// Running accumulator for one aggregate over one group.
pub(super) struct AggregateAcc {
    /// Rows seen (the `count` value).
    count: u64,
    /// Running sum of the numeric values seen.
    sum: f64,
    /// How many values parsed as numbers (the `avg` divisor).
    numeric: u64,
    min: Option<f64>,
    max: Option<f64>,
}

impl AggregateAcc {
    pub(super) const fn new(_spec: &DocAggregateSpec) -> Self {
        Self {
            count: 0,
            sum: 0.0,
            numeric: 0,
            min: None,
            max: None,
        }
    }

    /// Fold one document into this accumulator. `count` counts every row; the
    /// numeric aggregates parse `spec.field` and skip rows that are absent or
    /// non-numeric (an honest partial sum, never a fabricated zero).
    pub(super) fn observe(&mut self, spec: &DocAggregateSpec, doc: &Value) {
        self.count += 1;
        if spec.func == "count" {
            return;
        }
        let Some(value) = aggregate_number(doc.get(&spec.field)) else {
            return;
        };
        self.numeric += 1;
        self.sum += value;
        self.min = Some(self.min.map_or(value, |m| m.min(value)));
        self.max = Some(self.max.map_or(value, |m| m.max(value)));
    }

    /// Render the accumulator for `func` ("count"/"sum"/"avg"/"min"/"max").
    // `numeric` counts rows, so its magnitude is bounded by the scan cap and the
    // `u64 -> f64` cast is exact in practice.
    #[allow(clippy::cast_precision_loss)]
    pub(super) fn render(&self, func: &str) -> String {
        match func {
            "count" => self.count.to_string(),
            "sum" => render_number(self.sum),
            "avg" => {
                if self.numeric == 0 {
                    String::new()
                } else {
                    render_number(self.sum / self.numeric as f64)
                }
            }
            "min" => self.min.map_or_else(String::new, render_number),
            "max" => self.max.map_or_else(String::new, render_number),
            _ => String::new(),
        }
    }
}

/// Numeric view of a document field for the numeric aggregates: a JSON number,
/// or a string that parses as one.
fn aggregate_number(value: Option<&Value>) -> Option<f64> {
    let value = value?;
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
}

/// Canonical decimal rendering: whole numbers without a trailing `.0`, other
/// finite numbers as-is.
fn render_number(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        value.to_string()
    }
}

/// Default result key for an aggregate that specified no alias.
pub(super) fn default_aggregate_alias(spec: &DocAggregateSpec) -> String {
    if spec.func == "count" {
        "count".to_string()
    } else if spec.field.is_empty() {
        spec.func.clone()
    } else {
        format!("{}_{}", spec.func, spec.field)
    }
}
