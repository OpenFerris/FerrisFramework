//! Report export formats.

use ferris_core::{Error, Result, SerializationError, Value};
use serde::{Deserialize, Serialize};

use crate::query::ReportResult;

/// Supported export formats.
///
/// CSV, TSV, Markdown, HTML and JSON are implemented with in-tree code.
/// Parquet/Excel/PDF require heavy optional dependencies and are left for a
/// later phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    Csv,
    Tsv,
    Markdown,
    Html,
    Json,
}

/// Serialise a report result in `format`.
pub fn export_rows(result: &ReportResult, format: ExportFormat) -> Result<Vec<u8>> {
    match format {
        ExportFormat::Csv => Ok(export_csv(result).into_bytes()),
        ExportFormat::Tsv => Ok(export_tsv(result).into_bytes()),
        ExportFormat::Markdown => Ok(export_markdown(result).into_bytes()),
        ExportFormat::Html => Ok(export_html(result).into_bytes()),
        ExportFormat::Json => {
            let rows: Vec<serde_json::Value> = result
                .rows
                .iter()
                .map(|row| {
                    let mut obj = serde_json::Map::new();
                    for (column, value) in result.columns.iter().zip(row.iter()) {
                        obj.insert(column.fieldname.clone(), value.to_json());
                    }
                    serde_json::Value::Object(obj)
                })
                .collect();
            serde_json::to_vec(&rows).map_err(|e| Error::Serialization(SerializationError::Json(e)))
        }
    }
}

fn export_tsv(result: &ReportResult) -> String {
    let mut out = String::new();
    out.push_str(
        &result
            .columns
            .iter()
            .map(|c| tsv_cell(&c.label))
            .collect::<Vec<_>>()
            .join("\t"),
    );
    out.push('\n');
    for row in &result.rows {
        out.push_str(&row.iter().map(tsv_value).collect::<Vec<_>>().join("\t"));
        out.push('\n');
    }
    out
}

fn tsv_value(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(s) => tsv_cell(s),
        other => tsv_cell(&other.to_json().to_string()),
    }
}

fn tsv_cell(value: &str) -> String {
    let value = neutralise_formula(value);
    value.replace(['\t', '\n', '\r'], " ")
}

fn export_markdown(result: &ReportResult) -> String {
    let header = result
        .columns
        .iter()
        .map(|c| markdown_cell(&c.label))
        .collect::<Vec<_>>()
        .join(" | ");
    let separator = result
        .columns
        .iter()
        .map(|_| "---")
        .collect::<Vec<_>>()
        .join(" | ");
    let mut out = format!("| {header} |\n| {separator} |\n");
    for row in &result.rows {
        let cells = row
            .iter()
            .map(markdown_value)
            .collect::<Vec<_>>()
            .join(" | ");
        out.push_str(&format!("| {cells} |\n"));
    }
    out
}

fn markdown_value(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(s) => markdown_cell(s),
        other => markdown_cell(&other.to_json().to_string()),
    }
}

fn markdown_cell(value: &str) -> String {
    value.replace('|', "\\|").replace('\n', " ")
}

fn export_html(result: &ReportResult) -> String {
    let mut out = String::from("<table><thead><tr>");
    for column in &result.columns {
        out.push_str(&format!("<th>{}</th>", escape_html(&column.label)));
    }
    out.push_str("</tr></thead><tbody>");
    for row in &result.rows {
        out.push_str("<tr>");
        for value in row {
            let text = match value {
                Value::Null => String::new(),
                Value::String(s) => s.clone(),
                other => other.to_json().to_string(),
            };
            out.push_str(&format!("<td>{}</td>", escape_html(&text)));
        }
        out.push_str("</tr>");
    }
    out.push_str("</tbody></table>");
    out
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn export_csv(result: &ReportResult) -> String {
    let mut out = String::new();
    out.push_str(
        &result
            .columns
            .iter()
            .map(|c| csv_cell(&c.label))
            .collect::<Vec<_>>()
            .join(","),
    );
    out.push('\n');
    for row in &result.rows {
        out.push_str(&row.iter().map(csv_value).collect::<Vec<_>>().join(","));
        out.push('\n');
    }
    out
}

fn csv_value(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(s) => csv_cell(s),
        Value::Bool(b) => b.to_string(),
        // A negative number opens with `-`, which a spreadsheet evaluates; the
        // TSV path already neutralises it, so CSV must too (RPT7).
        Value::Number(n) => csv_cell(&n.to_string()),
        other => csv_cell(&other.to_json().to_string()),
    }
}

fn csv_cell(value: &str) -> String {
    let value = neutralise_formula(value);
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value
    }
}

/// Neutralise spreadsheet formula injection (CWE-1236).
///
/// RFC 4180 quoting alone does not protect the consumer: a cell opening with
/// `=`, `+`, `-`, `@` (or a leading tab/CR, which a spreadsheet also treats as
/// formula-adjacent) is evaluated when the exported file is opened. Prefixing
/// such a cell with a single quote preserves the value while making it inert.
/// This mirrors the client-side export in
/// `ferris-web/src/api/export.rs::push_csv_field`, which already defended
/// against the same attack.
fn neutralise_formula(value: &str) -> String {
    if value
        .trim_start()
        .starts_with(['=', '+', '-', '@', '\t', '\r'])
    {
        format!("'{value}")
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::ReportColumn;

    fn result() -> ReportResult {
        ReportResult {
            columns: vec![
                ReportColumn::new("name", "ID", "Data"),
                ReportColumn::new("title", "Title", "Data"),
            ],
            rows: vec![
                vec![Value::from("TODO-1"), Value::from("Buy, milk")],
                vec![Value::from("TODO-2"), Value::from("Say \"hi\"")],
            ],
        }
    }

    #[test]
    fn test_csv_export_escapes() {
        let csv = String::from_utf8(export_rows(&result(), ExportFormat::Csv).unwrap()).unwrap();
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines[0], "ID,Title");
        assert_eq!(lines[1], "TODO-1,\"Buy, milk\"");
        assert_eq!(lines[2], "TODO-2,\"Say \"\"hi\"\"\"");
    }

    #[test]
    fn test_csv_neutralises_formula_injection() {
        // CWE-1236: a cell opening with `=`, `+`, `-` or `@` must be made inert.
        let rows = ReportResult {
            columns: vec![ReportColumn::new("v", "V", "Data")],
            rows: vec![
                vec![Value::from("=cmd|'/c calc'!A1")],
                vec![Value::from("+1+1")],
                vec![Value::from("-2-2")],
                vec![Value::from("@SUM(A1)")],
                vec![Value::from("safe value")],
            ],
        };
        let csv = String::from_utf8(export_rows(&rows, ExportFormat::Csv).unwrap()).unwrap();
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines[1], "'=cmd|'/c calc'!A1");
        assert_eq!(lines[2], "'+1+1");
        assert_eq!(lines[3], "'-2-2");
        assert_eq!(lines[4], "'@SUM(A1)");
        assert_eq!(lines[5], "safe value"); // untouched when not formula-adjacent

        // TSV has the same spreadsheet evaluation path.
        let tsv = String::from_utf8(export_rows(&rows, ExportFormat::Tsv).unwrap()).unwrap();
        assert!(tsv.contains("'=cmd|'/c calc'!A1"));
        assert!(tsv.contains("safe value"));
    }

    #[test]
    fn test_json_export() {
        let json = export_rows(&result(), ExportFormat::Json).unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&json).unwrap();
        assert_eq!(parsed[0]["name"], "TODO-1");
        assert_eq!(parsed[1]["title"], "Say \"hi\"");
    }

    #[test]
    fn test_tsv_markdown_and_html_exports() {
        let tsv = String::from_utf8(export_rows(&result(), ExportFormat::Tsv).unwrap()).unwrap();
        assert!(tsv.starts_with("ID\tTitle\n"));
        assert!(tsv.contains("TODO-1\tBuy, milk"));

        let md =
            String::from_utf8(export_rows(&result(), ExportFormat::Markdown).unwrap()).unwrap();
        assert!(md.starts_with("| ID | Title |\n| --- | --- |\n"));
        assert!(md.contains("| TODO-1 | Buy, milk |"));

        let html = String::from_utf8(export_rows(&result(), ExportFormat::Html).unwrap()).unwrap();
        assert!(html.contains("<th>Title</th>"));
        assert!(html.contains("<td>Say &quot;hi&quot;</td>"));
    }
}
