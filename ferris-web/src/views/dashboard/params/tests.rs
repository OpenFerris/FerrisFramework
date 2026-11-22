use leptos::prelude::*;

use super::fieldtype::{PARAM_FIELDTYPES, options_value, param_fieldtype};
use super::panel::param_to_field;
use super::*;
use crate::views::services::validate_form;
use ferris_schema::contract::{WidgetInstance, WidgetParam};

fn param(name: &str, value_type: &str) -> WidgetParam {
    WidgetParam {
        name: name.to_string(),
        value_type: value_type.to_string(),
        ..Default::default()
    }
}

fn instance(params: Vec<(&str, &str)>) -> WidgetInstance {
    WidgetInstance {
        params: params
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        ..Default::default()
    }
}

/// A declared fieldtype wins; a non-empty `options` means select; the
/// legacy kinds map to their fieldtype; unknown vocabulary degrades to
/// `Data`; a fieldtype the panel cannot draw without a document is never
/// selected.
#[test]
fn fieldtype_mapping_prefers_declared_fieldtype_then_options_then_kind() {
    assert_eq!(param_fieldtype(&param("t", "Text")), "Text");
    assert_eq!(param_fieldtype(&param("l", "Link")), "Link");
    assert_eq!(param_fieldtype(&param("d", "doctype")), "Link");
    assert_eq!(param_fieldtype(&param("n", "number")), "Float");
    assert_eq!(param_fieldtype(&param("b", "bool")), "Check");
    assert_eq!(param_fieldtype(&param("j", "json")), "JSON");
    assert_eq!(param_fieldtype(&param("s", "string")), "Data");

    let mut select = param("docstatus", "string");
    select.options = vec!["0".into(), "1".into(), "all".into()];
    assert_eq!(param_fieldtype(&select), "Select");

    // `Table MultiSelect` draws as the multi-value chips control, and its
    // canonical spelling wins over the "non-empty options ⇒ Select" rule
    // (its `options` hold the *target doctype*, not the choices).
    let mut multi = param("series", "Table MultiSelect");
    multi.options = vec!["Magnitud".into()];
    assert_eq!(param_fieldtype(&multi), "Table MultiSelect");

    // Document-bound fieldtypes are not renderable here: degrade.
    assert_eq!(param_fieldtype(&param("btn", "Button")), "Data");
    assert_eq!(param_fieldtype(&param("gal", "Attachment Gallery")), "Data");

    // Layout markers are real controls in the panel: they declare its
    // shape instead of holding a value.
    for ft in [
        "Tab Break",
        "Section Break",
        "Column Break",
        "Heading",
        "HTML",
        "Read Only",
    ] {
        assert_eq!(param_fieldtype(&param("marker", ft)), ft);
    }

    // The rest of the form view's editable catalog is accepted as-is.
    for ft in [
        "Text Editor",
        "Markdown Editor",
        "Rating",
        "Duration",
        "Week",
        "Geolocation",
        "Signature",
        "Barcode",
        "Image",
        "File",
        "Attach",
        "Attach Image",
        "Dynamic Link",
    ] {
        assert_eq!(
            param_fieldtype(&param("p", ft)),
            ft,
            "{ft} must map to itself"
        );
    }
}

/// Every fieldtype the form view lets a user add is available for a widget
/// parameter too, except the one that is document-bound. `Table
/// MultiSelect` is the reverse: a widget-only addition, because a settings
/// panel needs a multi-value control the customize dialog never offers.
///
/// This is the guard that keeps the two catalogs from drifting: adding a
/// type to the form view without deciding its parameter behaviour fails
/// here, instead of silently degrading the param to `Data`.
#[test]
fn params_offer_every_editable_form_fieldtype() {
    use crate::components::views::form::customize::FIELD_TYPES;

    // Document-bound: its files are keyed by the document, which a widget
    // settings dialog does not have. `Secret` is excluded for a different
    // reason: a dashboard parameter is stored in the board layout, not in
    // an encrypted `Secret` field, so it must never be offered as one.
    // `Widget` hosts another widget: a parameter holds a value, and a widget
    // placed inside a widget's settings panel has none (it would also nest the
    // host into itself with no document context to resolve against).
    const NEVER_A_PARAM: &[&str] = &["Attachment Gallery", "Secret", "Widget"];

    for ft in FIELD_TYPES {
        if NEVER_A_PARAM.contains(ft) {
            assert!(
                !PARAM_FIELDTYPES.contains(ft),
                "{ft} must stay out of the parameter panel"
            );
        } else {
            assert!(
                PARAM_FIELDTYPES.contains(ft),
                "form fieldtype {ft} is missing from the widget parameter panel"
            );
        }
    }
    assert!(PARAM_FIELDTYPES.contains(&"Table MultiSelect"));
}

/// `Select` options arrive newline-joined (the `DynField` shape), a
/// legacy `doctype` param always links `DocType`, an explicit `Link` (or
/// `Table MultiSelect`) takes its target from the first option.
#[test]
fn options_reach_the_control_in_its_expected_shape() {
    let mut select = param("docstatus", "string");
    select.options = vec!["0".into(), "1".into()];
    assert_eq!(options_value(&select, "Select"), Some("0\n1".to_string()));
    assert_eq!(
        options_value(&param("d", "doctype"), "Link").as_deref(),
        Some("DocType")
    );

    let mut link = param("owner", "Link");
    link.options = vec!["User".into(), "ignored".into()];
    assert_eq!(options_value(&link, "Link").as_deref(), Some("User"));

    // The multi-value control autocompletes the same target doctype.
    let mut multi = param("series", "Table MultiSelect");
    multi.options = vec!["Magnitud".into()];
    assert_eq!(
        options_value(&multi, "Table MultiSelect").as_deref(),
        Some("Magnitud")
    );

    // `Currency`/`Barcode` read their first option as the prefix/symbology
    // the renderer already takes from `options`.
    let mut currency = param("precio", "Currency");
    currency.options = vec!["EUR".into()];
    assert_eq!(options_value(&currency, "Currency").as_deref(), Some("EUR"));
    let mut barcode = param("codigo", "Barcode");
    barcode.options = vec!["EAN13".into()];
    assert_eq!(options_value(&barcode, "Barcode").as_deref(), Some("EAN13"));
}

/// A `Table MultiSelect` parameter seeds from the stored JSON array and
/// writes it back unchanged: the value is the shape the widget's JS parses
/// (`["a","b"]`), never a comma-separated string the operator has to type.
#[test]
fn multi_select_param_round_trips_the_json_array() {
    let _owner = crate::test_support::owner();
    let mut series = param("series", "Table MultiSelect");
    series.options = vec!["Magnitud".into()];

    // Seeded from what a previous edit stored.
    let widget = instance(vec![("series", r#"["pressure","flow"]"#)]);
    let fields = [param_to_field(&series, &widget)];
    assert_eq!(fields[0].fieldtype, "Table MultiSelect");
    assert_eq!(fields[0].options.as_deref(), Some("Magnitud"));
    assert_eq!(fields[0].value.get(), r#"["pressure","flow"]"#);

    // Editing and applying writes the same JSON array back.
    let mut widget = instance(vec![("series", r#"["pressure"]"#)]);
    let fields = [param_to_field(&series, &widget)];
    fields[0].value.set(r#"["pressure","flow"]"#.to_string());
    sync_params(&mut widget, &fields, std::slice::from_ref(&series));
    assert_eq!(
        widget.params,
        vec![("series".to_string(), r#"["pressure","flow"]"#.to_string())]
    );
}

/// The stored value wins over the default, the default is what an unset
/// parameter displays, and a `Check` seeds from `"1"`.
#[test]
fn field_seeds_from_instance_value_then_default() {
    let _owner = crate::test_support::owner();
    let mut with_default = param("page-size", "number");
    with_default.default = Some("10".into());
    let field = param_to_field(&with_default, &instance(vec![("page-size", "25")]));
    assert_eq!(field.fieldtype, "Float");
    assert_eq!(field.value.get(), "25");

    let field = param_to_field(&with_default, &instance(vec![]));
    assert_eq!(field.value.get(), "10");

    let mut flag = param("dense", "bool");
    flag.default = Some("1".into());
    let field = param_to_field(&flag, &instance(vec![("dense", "0")]));
    assert_eq!(field.fieldtype, "Check");
    assert!(!field.checked.get());
    let field = param_to_field(&flag, &instance(vec![]));
    assert!(field.checked.get());
}

/// The materialization rules: an untouched default is not written, a
/// changed value is, an empty value removes the key, and a `Check` only
/// stores `"0"` when it has to override a `"1"` default.
#[test]
fn sync_only_writes_what_the_user_changed() {
    let _owner = crate::test_support::owner();
    let mut default_param = param("page-size", "number");
    default_param.default = Some("10".into());
    let mut widget = instance(vec![]);
    let fields = vec![param_to_field(&default_param, &widget)];
    sync_params(&mut widget, &fields, std::slice::from_ref(&default_param));
    assert!(
        widget.params.is_empty(),
        "untouched default must not be materialized"
    );

    // Edited → upserted.
    let mut widget = instance(vec![]);
    let fields = vec![param_to_field(&default_param, &widget)];
    fields[0].value.set("25".to_string());
    sync_params(&mut widget, &fields, std::slice::from_ref(&default_param));
    assert_eq!(
        widget.params,
        vec![("page-size".to_string(), "25".to_string())]
    );

    // Cleared → key removed so the default applies again.
    let mut widget = instance(vec![("page-size", "25")]);
    let fields = vec![param_to_field(&default_param, &widget)];
    fields[0].value.set(String::new());
    sync_params(&mut widget, &fields, std::slice::from_ref(&default_param));
    assert!(widget.params.is_empty());

    // A stored value equal to the default stays stored (the user picked it).
    let mut widget = instance(vec![("page-size", "10")]);
    let fields = vec![param_to_field(&default_param, &widget)];
    sync_params(&mut widget, &fields, std::slice::from_ref(&default_param));
    assert_eq!(
        widget.params,
        vec![("page-size".to_string(), "10".to_string())]
    );
}

/// `Check` rules: checked writes `"1"`; unchecked only writes `"0"` to
/// override a `"1"` default, otherwise the key disappears.
#[test]
fn sync_check_stores_1_or_overrides_the_default() {
    let _owner = crate::test_support::owner();
    let mut flag = param("dense", "bool");
    flag.default = Some("1".into());

    let mut widget = instance(vec![]);
    let fields = vec![param_to_field(&flag, &widget)];
    fields[0].checked.set(false);
    sync_params(&mut widget, &fields, std::slice::from_ref(&flag));
    assert_eq!(widget.params, vec![("dense".to_string(), "0".to_string())]);

    let mut widget = instance(vec![("dense", "1")]);
    let fields = vec![param_to_field(&flag, &widget)];
    fields[0].checked.set(false);
    sync_params(&mut widget, &fields, std::slice::from_ref(&flag));
    assert_eq!(widget.params, vec![("dense".to_string(), "0".to_string())]);

    let mut no_default = param("dense", "bool");
    no_default.default = None;
    let mut widget = instance(vec![("dense", "1")]);
    let fields = vec![param_to_field(&no_default, &widget)];
    fields[0].checked.set(false);
    sync_params(&mut widget, &fields, std::slice::from_ref(&no_default));
    assert!(widget.params.is_empty());

    let mut widget = instance(vec![]);
    let fields = vec![param_to_field(&no_default, &widget)];
    fields[0].checked.set(true);
    sync_params(&mut widget, &fields, std::slice::from_ref(&no_default));
    assert_eq!(widget.params, vec![("dense".to_string(), "1".to_string())]);
}

/// The panel's validation path is the form's: a required empty param and
/// a non-numeric `Float` fail, a valid set passes.
#[test]
fn validate_params_uses_the_form_rules() {
    let _owner = crate::test_support::owner();
    let mut required = param("doctype", "doctype");
    required.required = true;
    let widget = instance(vec![]);
    let fields = vec![param_to_field(&required, &widget)];
    assert!(!validate_form(&fields));

    fields[0].value.set("Invoice".into());
    assert!(validate_form(&fields));

    let count = param("page-size", "number");
    let widget = instance(vec![("page-size", "abc")]);
    let fields = vec![param_to_field(&count, &widget)];
    assert!(!validate_form(&fields));
}

/// A definition that declares no layout marker keeps the historical
/// `group` sections: one synthesized `Section Break` per group, in
/// declaration order, with ungrouped parameters under "Ajustes".
#[test]
fn legacy_groups_become_sections() {
    let _owner = crate::test_support::owner();
    let mut first = param("doctype", "doctype");
    first.group = Some("Datos".into());
    let mut second = param("page-size", "number");
    second.group = Some("Datos".into());
    let mut third = param("color", "Color");
    third.group = Some("Presentación".into());
    let fourth = param("dense", "bool");

    let fields = param_fields(&[first, second, third, fourth], &instance(vec![]));
    let shape: Vec<(&str, &str)> = fields
        .iter()
        .map(|field| (field.fieldtype.as_str(), field.label.as_str()))
        .collect();
    assert_eq!(
        shape,
        vec![
            ("Section Break", "Datos"),
            ("Link", "doctype"),
            ("Float", "page-size"),
            ("Section Break", "Presentación"),
            ("Color", "color"),
            ("Section Break", "Ajustes"),
            ("Check", "dense"),
        ]
    );
}

/// A declared `Section Break`/`Tab Break`/`Column Break` replaces the
/// `group` fallback; the parameters are mapped one-to-one, in order, so
/// the widget owns the whole shape of its panel.
#[test]
fn declared_layout_wins_over_groups() {
    let _owner = crate::test_support::owner();
    let mut tab = param("datos", "Tab Break");
    tab.label = Some("Datos".into());
    let mut first = param("doctype", "doctype");
    first.group = Some("Ignored".into());
    let mut section = param("presentacion", "Section Break");
    section.label = Some("Presentación".into());
    let mut heading = param("nota", "Heading");
    heading.label = Some("Ajustes finos".into());
    let column = param("columna", "Column Break");
    let second = param("page-size", "number");

    let fields = param_fields(
        &[tab, first, section, heading, column, second],
        &instance(vec![]),
    );
    let shape: Vec<&str> = fields
        .iter()
        .map(|field| field.fieldtype.as_str())
        .collect();
    assert_eq!(
        shape,
        vec![
            "Tab Break",
            "Link",
            "Section Break",
            "Heading",
            "Column Break",
            "Float",
        ]
    );

    // The tab label is the human one, not the kebab-case key.
    let (tabs, default, _) = crate::views::services::form_layout(&fields);
    assert_eq!(tabs, vec![("datos".to_string(), "Datos".to_string())]);
    assert_eq!(default, "datos");
}

/// A `hidden` parameter never reaches the panel, and "Aplicar" does not
/// touch its stored value because it is not in the field list.
#[test]
fn hidden_params_stay_out_but_keep_their_stored_value() {
    let _owner = crate::test_support::owner();
    let mut hidden = param("internal", "Text");
    hidden.hidden = true;
    let shown = param("visible", "Text");
    let mut widget = instance(vec![("internal", "keep-me")]);
    let fields = param_fields(&[hidden, shown], &widget);
    let shape: Vec<(&str, &str)> = fields
        .iter()
        .map(|field| (field.fieldtype.as_str(), field.fieldname.as_str()))
        .collect();
    assert_eq!(
        shape,
        vec![("Section Break", ""), ("Text", "visible")],
        "the hidden parameter must not reach the panel"
    );

    sync_params(
        &mut widget,
        &fields,
        &[param("internal", "Text"), param("visible", "Text")],
    );
    assert_eq!(
        widget.params,
        vec![("internal".to_string(), "keep-me".to_string())]
    );
}

/// The form properties a parameter can now declare reach the control:
/// `read_only`, `placeholder` and the `depends_on` expression.
#[test]
fn param_form_properties_reach_the_field() {
    let _owner = crate::test_support::owner();
    let mut param = param("page-size", "number");
    param.read_only = true;
    param.placeholder = Some("10".into());
    param.depends_on = Some("doc.kind == 'table'".into());

    let field = param_to_field(&param, &instance(vec![]));
    assert!(field.read_only);
    assert_eq!(field.placeholder.as_deref(), Some("10"));
    assert_eq!(field.depends_on.as_deref(), Some("doc.kind == 'table'"));
}

/// Layout markers and display-only types are never written back, even
/// when they carry a `default` (a marker must not create a `("", "")` key).
#[test]
fn sync_skips_layout_and_display_only_params() {
    let _owner = crate::test_support::owner();
    let mut section = param("datos", "Section Break");
    section.default = Some("stray".into());
    let mut heading = param("nota", "Heading");
    heading.default = Some("stray".into());
    let mut read_only = param("mirror", "Read Only");
    read_only.default = Some("stray".into());
    let value = param("page-size", "number");

    let mut widget = instance(vec![]);
    let fields = param_fields(&[section, heading, read_only, value], &instance(vec![]));
    sync_params(&mut widget, &fields, &[]);
    assert!(widget.params.is_empty());
}
