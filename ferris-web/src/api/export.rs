//! CSV export of the current view (download on hydrate, no-op on SSR).

use serde_json::Value;

/// Download the given rows as CSV.
pub fn export_csv(doctype: &str, columns: &[crate::api::Column], rows: &[Value]) {
    // Built inside the wasm arm: on the server there is no download, so
    // rendering the whole document first would be wasted work on every call.
    #[cfg(target_arch = "wasm32")]
    download_blob(
        &format!("{doctype}.csv"),
        "text/csv",
        &build_csv(columns, rows),
    );

    #[cfg(not(target_arch = "wasm32"))]
    {
        leptos::logging::log!(
            "CSV export skipped during SSR for {} ({} columns, {} rows)",
            doctype,
            columns.len(),
            rows.len()
        );
    }
}

/// Render the rows as a CSV document. Only the browser builds one, but the
/// tests exercise it on native too.
#[cfg(any(target_arch = "wasm32", test))]
fn build_csv(columns: &[crate::api::Column], rows: &[Value]) -> String {
    if columns.is_empty() {
        return String::new();
    }

    let mut csv = String::with_capacity(rows.len() * columns.len() * 16);
    for (i, column) in columns.iter().enumerate() {
        if i > 0 {
            csv.push(',');
        }
        push_csv_field(&mut csv, &column.label);
    }
    csv.push('\n');

    for row in rows {
        for (i, column) in columns.iter().enumerate() {
            if i > 0 {
                csv.push(',');
            }
            let value = row.get(&column.fieldname).unwrap_or(&Value::Null);
            push_csv_field(
                &mut csv,
                &crate::api::display_value(&column.fieldtype, value),
            );
        }
        csv.push('\n');
    }
    csv
}

/// Append one quoted CSV field.
///
/// Quoting alone is not enough: a field opening with `=`, `+`, `-` or `@` is
/// run as a formula when the file is opened, and labels come from doctype
/// metadata that no review pass necessarily sees.
#[cfg(any(target_arch = "wasm32", test))]
fn push_csv_field(out: &mut String, raw: &str) {
    out.push('"');
    if raw
        .trim_start()
        .starts_with(['=', '+', '-', '@', '\t', '\r'])
    {
        out.push('\'');
    }
    for ch in raw.chars() {
        if ch == '"' {
            out.push('"');
        }
        out.push(ch);
    }
    out.push('"');
}

#[cfg(target_arch = "wasm32")]
fn download_blob(filename: &str, mime: &str, content: &str) {
    use wasm_bindgen::JsCast;
    use wasm_bindgen::closure::Closure;
    use web_sys::{Blob, BlobPropertyBag, HtmlAnchorElement, Url};

    let opts = BlobPropertyBag::new();
    opts.set_type(mime);
    let parts = js_sys::Array::new();
    parts.push(&wasm_bindgen::JsValue::from_str(content));

    // No panic on this path: it runs from a button on the list/report, and an
    // exception here tears down the whole wasm module. A browser that refuses
    // the blob simply gets no file.
    let Ok(blob) = Blob::new_with_str_sequence_and_options(&parts, &opts) else {
        return;
    };
    let Ok(url) = Url::create_object_url_with_blob(&blob) else {
        return;
    };

    let Some(window) = web_sys::window() else {
        return;
    };
    let Some(document) = window.document() else {
        return;
    };
    let Ok(element) = document.create_element("a") else {
        return;
    };
    let Ok(anchor) = element.dyn_into::<HtmlAnchorElement>() else {
        return;
    };
    anchor.set_href(&url);
    anchor.set_download(filename);
    anchor.click();

    // Revoking in the same task cancels the download before the browser has read
    // the blob, so hand the URL back once the current task has yielded. The
    // timer needs the closure to outlive this call, and `set_timeout` takes a
    // borrow, so the closure is intentionally leaked: `once` means it runs a
    // single time and is then dead weight, and this path only runs when the
    // user asks for a file.
    let revoke = url.clone();
    let cleanup = Closure::once(move || {
        let _ = Url::revoke_object_url(&revoke);
    });
    if window
        .set_timeout_with_callback_and_timeout_and_arguments_0(cleanup.as_ref().unchecked_ref(), 0)
        .is_err()
    {
        // No timer: drop the closure rather than leak it, at the cost of
        // leaving the object URL alive for the page's lifetime.
        return;
    }
    cleanup.forget();
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn column(fieldname: &str, label: &str) -> crate::api::Column {
        crate::api::Column {
            fieldname: fieldname.to_string(),
            label: label.to_string(),
            width: None,
            sortable: false,
            fieldtype: "Data".to_string(),
            options: None,
        }
    }

    #[test]
    fn headers_are_quoted_like_the_cells() {
        let csv = build_csv(&[column("total", "Total, in EUR")], &[]);
        assert_eq!(csv, "\"Total, in EUR\"\n");
    }

    #[test]
    fn separators_inside_a_field_do_not_shift_columns() {
        let csv = build_csv(
            &[column("name", "Name"), column("note", "Note")],
            &[json!({ "name": "a,b", "note": "plain" })],
        );
        assert_eq!(csv, "\"Name\",\"Note\"\n\"a,b\",\"plain\"\n");
    }

    #[test]
    fn quotes_are_doubled() {
        let csv = build_csv(&[column("n", "N")], &[json!({ "n": "say \"hi\"" })]);
        assert_eq!(csv, "\"N\"\n\"say \"\"hi\"\"\"\n");
    }

    #[test]
    fn leading_operators_cannot_run_as_a_formula() {
        let csv = build_csv(
            &[column("v", "V")],
            &[
                json!({ "v": "=1+1" }),
                json!({ "v": "@SUM(A1)" }),
                json!({ "v": "-2+3" }),
            ],
        );
        assert_eq!(csv, "\"V\"\n\"'=1+1\"\n\"'@SUM(A1)\"\n\"'-2+3\"\n");
    }

    #[test]
    fn no_columns_means_no_document() {
        assert_eq!(build_csv(&[], &[json!({ "a": 1 })]), "");
    }

    #[test]
    fn a_missing_field_becomes_an_empty_field() {
        let csv = build_csv(
            &[column("there", "There"), column("absent", "Absent")],
            &[json!({})],
        );
        assert_eq!(csv, "\"There\",\"Absent\"\n\"\",\"\"\n");
    }
}
