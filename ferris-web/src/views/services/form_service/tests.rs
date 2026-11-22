use super::*;

/// Run `body` inside an owner (and its `Arena`).
///
/// `RwSignal` in this workspace is built with `sandboxed-arenas` active
/// (feature unification across the workspace), so a signal read needs a
/// live `Arena` — an owner alone is not enough, and a bare `#[test]` panics
/// with "no Arena is active". `cargo test -p ferris-web --lib` alone does
/// not enable the feature, which is why this only bites in a workspace run.
/// [`crate::test_support::owner`] is the shared implementation.
fn in_owner(body: impl FnOnce()) {
    let _owner = crate::test_support::owner();
    body();
}

fn field(fieldname: &str, fieldtype: &str, value: &str) -> FormField {
    FormField {
        fieldname: fieldname.to_string(),
        label: fieldname.to_string(),
        fieldtype: fieldtype.to_string(),
        options: None,
        reqd: false,
        read_only: false,
        placeholder: None,
        description: None,
        min_value: None,
        max_value: None,
        bold: false,
        max_length: None,
        precision: None,
        fetch_from: None,
        fetch_if_empty: false,
        is_name: false,
        depends_on: None,
        value: RwSignal::new(value.to_string()),
        checked: RwSignal::new(false),
        error: RwSignal::new(None),
    }
}

/// The copy must not be the document it came from, nor sit in the same place
/// in its workflow: `name` and `docstatus` are the two keys that would do
/// either. Everything else the engine owns is not a form field, so it never
/// reaches the payload in the first place.
#[test]
fn duplicate_payload_detaches_identity_and_docstatus() {
    in_owner(|| {
        let fields = vec![
            field("name", "Data", "TASK00001"),
            field("status", "Select", "Open"),
            field("priority", "Select", "Low"),
        ];

        let map = duplicate_payload(&fields)
            .as_object()
            .expect("payload is an object")
            .clone();

        assert!(!map.contains_key("name"), "la copia arrastra el name");
        assert!(
            !map.contains_key("docstatus"),
            "la copia arrastra docstatus"
        );
        assert_eq!(map.get("status").and_then(Value::as_str), Some("Open"));
        assert_eq!(map.get("priority").and_then(Value::as_str), Some("Low"));
    });
}

/// The fields the engine sets by itself are absent because they are not form
/// fields. Asserted explicitly because "absent by construction" is exactly the
/// kind of claim that stops being true when someone adds a field to the meta.
#[test]
fn duplicate_payload_carries_no_system_fields() {
    in_owner(|| {
        let fields = vec![field("subject", "Data", "Tarea")];
        let map = duplicate_payload(&fields)
            .as_object()
            .expect("payload is an object")
            .clone();

        for key in [
            "name",
            "docstatus",
            "owner",
            "creation",
            "modified",
            "modified_by",
            "idx",
            "_user_tags",
            "amended_from",
        ] {
            assert!(!map.contains_key(key), "{key} no debe viajar en la copia");
        }
        assert_eq!(map.get("subject").and_then(Value::as_str), Some("Tarea"));
    });
}

/// The dirty fingerprint is taken from the same payload a save would send, so
/// retyping a value back to what it was reads as pristine again — the form
/// must not claim unsaved changes it would in fact write nothing for.
#[test]
fn fingerprint_is_stable_across_equal_payloads() {
    in_owner(|| {
        let original = vec![field("status", "Select", "Open")];
        let baseline = payload_fingerprint(&original);

        let retyped = vec![field("status", "Select", "Open")];
        assert_eq!(payload_fingerprint(&retyped), baseline);

        let changed = vec![field("status", "Select", "Closed")];
        assert_ne!(payload_fingerprint(&changed), baseline);
    });
}

/// A `new` form leads with the doctype's id, prefilled from its naming
/// declaration, so the user sees what the document will be called.
#[test]
fn new_form_leads_with_an_id_field_prefilled_from_the_series() {
    use crate::api::DynMeta;
    in_owner(|| {
        let meta = DynMeta {
            name: "Task".to_string(),
            naming_series: "TASK-.#####".to_string(),
            ..Default::default()
        };
        let fields = build_form_fields(&meta, &Value::Null, true);
        let id = &fields[0];
        assert!(id.is_name, "el primer campo ha de ser el ID");
        assert_eq!(id.fieldname, "name");
        assert_eq!(id.value.get(), "TASK-.#####");
        assert!(!id.read_only, "una serie se puede sobrescribir a mano");

        // A stored document already has its name; the form must not invent one.
        let loaded = build_form_fields(&meta, &Value::Null, false);
        assert!(!loaded.iter().any(|f| f.is_name));
    });
}

/// The untouched series template must not travel in the payload: the engine
/// would otherwise use the literal `TASK-.#####` as the primary key instead of
/// minting the next name. A value the user typed is sent as-is.
#[test]
fn an_untouched_series_template_is_omitted_from_the_payload() {
    use crate::api::DynMeta;
    in_owner(|| {
        let meta = DynMeta {
            name: "Task".to_string(),
            naming_series: "TASK-.#####".to_string(),
            ..Default::default()
        };
        let fields = build_form_fields(&meta, &Value::Null, true);
        assert!(build_form_payload(&fields).get("name").is_none());

        fields[0].value.set("TASK-CUSTOM".to_string());
        assert_eq!(
            build_form_payload(&fields)
                .get("name")
                .and_then(Value::as_str),
            Some("TASK-CUSTOM")
        );
    });
}

/// `Prompt` naming moves the requirement onto the user: the id is required, and
/// validation only passes once it is filled.
#[test]
fn prompt_naming_makes_the_id_required() {
    use crate::api::{DynMeta, NamingType};
    in_owner(|| {
        let meta = DynMeta {
            name: "Case".to_string(),
            naming_type: Some(NamingType::Prompt),
            ..Default::default()
        };
        let fields = build_form_fields(&meta, &Value::Null, true);
        assert!(fields[0].reqd);
        assert!(!fields[0].read_only);
        assert!(!validate_form(&fields), "un id vacío ha de fallar");

        fields[0].value.set("CASE-1".to_string());
        assert!(validate_form(&fields));
    });
}

/// Engine-owned naming (`Uuid`, `AutoIncrement`) shows the id read-only and
/// never sends it: the engine mints it.
#[test]
fn engine_owned_naming_shows_a_read_only_id() {
    use crate::api::{DynMeta, NamingType};
    in_owner(|| {
        for naming in [NamingType::Uuid, NamingType::AutoIncrement] {
            let meta = DynMeta {
                name: "Blob".to_string(),
                naming_type: Some(naming),
                ..Default::default()
            };
            let fields = build_form_fields(&meta, &Value::Null, true);
            assert!(fields[0].read_only, "{naming:?} ha de ser de solo lectura");
            assert!(build_form_payload(&fields).get("name").is_none());
        }
    });
}

/// A `Link` field with `fetch_from` is declared on the form; the effect that
/// performs the fetch reads the pair at runtime, but the declaration has to
/// survive the trip from metadata to the field model either way.
#[test]
fn fetch_from_is_carried_into_the_field_model() {
    use crate::api::{DynField, DynMeta};
    in_owner(|| {
        let meta = DynMeta {
            name: "Order".to_string(),
            fields: vec![
                DynField {
                    fieldname: "customer".to_string(),
                    fieldtype: "Link".to_string(),
                    options: Some("Customer".to_string()),
                    ..Default::default()
                },
                DynField {
                    fieldname: "customer_name".to_string(),
                    fieldtype: "Data".to_string(),
                    fetch_from: Some("customer.customer_name".to_string()),
                    fetch_if_empty: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let fields = build_form_fields(&meta, &Value::Null, false);
        let fetched = fields
            .iter()
            .find(|f| f.fieldname == "customer_name")
            .expect("el campo existe");
        assert_eq!(
            fetched.fetch_from.as_deref(),
            Some("customer.customer_name")
        );
        assert!(fetched.fetch_if_empty);
    });
}

/// a field whose metadata still carries a legacy camelCase
/// fieldtype (`SmallText` — what the macro emitted before the
/// canonicalization) is normalized on the way in, so the renderer's arms,
/// which match the wire spelling, still route it to its dedicated component
/// instead of the plain-input catch-all.
#[test]
fn legacy_fieldtype_aliases_are_canonicalized_on_the_way_in() {
    use crate::api::{DynField, DynMeta};
    in_owner(|| {
        let meta = DynMeta {
            name: "Order".to_string(),
            fields: vec![DynField {
                fieldname: "notes".to_string(),
                fieldtype: "SmallText".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let fields = build_form_fields(&meta, &Value::Null, false);
        assert_eq!(fields[0].fieldtype, "Small Text");
    });
}

/// an `HTML` field is content, not a marker — it must flow into
/// the layout as a renderable item (the renderer draws the block from its
/// `options`), instead of being dropped on the way as it was before.
#[test]
fn html_fields_flow_into_the_layout_as_renderable_items() {
    in_owner(|| {
        let fields = vec![
            field("seccion", "Section Break", ""),
            field("nota", "HTML", "<b>hola</b>"),
        ];
        let (_, sections) = build_form_layout(&fields);
        assert_eq!(sections.len(), 1);
        assert!(
            matches!(sections[0].items[0], FormSectionItem::Field(1)),
            "the HTML field must be the second item of the section"
        );
    });
}

/// A `Check` with `default = "1"` starts checked in a new document, and a stored
/// value always wins over that default.
///
/// The flag used to open unchecked in every new form and the first save wrote
/// `false`, silently flipping what the metadata had turned on.
#[test]
fn a_check_field_starts_checked_from_its_default() {
    use crate::api::{DynField, DynMeta};
    in_owner(|| {
        let meta = DynMeta {
            name: "Flag".to_string(),
            fields: vec![DynField {
                fieldname: "active".to_string(),
                fieldtype: "Check".to_string(),
                default: Some("1".to_string()),
                ..Default::default()
            }],
            ..Default::default()
        };

        let fresh = build_form_fields(&meta, &Value::Null, false);
        let active = fresh
            .iter()
            .find(|field| field.fieldname == "active")
            .expect("the Check field is in the form");
        assert!(
            active.checked.get_untracked(),
            "a Check default of 1 must start checked"
        );

        let stored = build_form_fields(&meta, &serde_json::json!({ "active": false }), false);
        let active = stored
            .iter()
            .find(|field| field.fieldname == "active")
            .expect("the Check field is in the form");
        assert!(
            !active.checked.get_untracked(),
            "a stored false wins over the declared default"
        );
    });
}

/// A `JSON` field is stored as JSON, not as the text the editor shows, and
/// invalid text is reported before the request instead of by the server.
///
/// The payload used to carry the raw string, which the engine rejects with
/// "Value must be a JSON object or array"; the message arrived after a
/// round-trip and never pointed at the field.
#[test]
fn a_json_field_round_trips_as_json_and_rejects_invalid_text() {
    in_owner(|| {
        let payload = build_form_payload(&[field("meta", "JSON", "{\"a\":1,\"b\":[2]}")]);
        assert_eq!(
            payload
                .get("meta")
                .and_then(|v| v.get("a"))
                .and_then(Value::as_i64),
            Some(1),
            "the object must travel as an object"
        );

        // A scalar is valid JSON but not a value the field accepts: omit it.
        let scalar = build_form_payload(&[field("meta", "JSON", "42")]);
        assert!(scalar.get("meta").is_none());

        let good = field("meta", "JSON", "[1,2]");
        assert!(validate_form(&[good]));

        let bad = field("meta", "JSON", "{oops}");
        let bad_error = bad.error;
        assert!(!validate_form(&[bad]));
        assert!(
            bad_error.get_untracked().is_some(),
            "the field must carry the reason"
        );
    });
}

/// The native `datetime-local` value must reach the engine as
/// `YYYY-MM-DD HH:MM:SS`; the raw `…T10:30` was rejected on every save.
#[test]
fn a_datetime_is_normalized_to_the_wire_format() {
    in_owner(|| {
        let payload = build_form_payload(&[field("when", "Datetime", "2026-12-24T10:30")]);
        assert_eq!(
            payload.get("when").and_then(Value::as_str),
            Some("2026-12-24 10:30:00")
        );

        let seconds = build_form_payload(&[field("when", "Datetime", "2026-12-24T10:30:15")]);
        assert_eq!(
            seconds.get("when").and_then(Value::as_str),
            Some("2026-12-24 10:30:15")
        );

        let offset = build_form_payload(&[field("when", "Datetime", "2026-12-24T10:30:00Z")]);
        assert_eq!(
            offset.get("when").and_then(Value::as_str),
            Some("2026-12-24 10:30:00")
        );
    });
}

/// A stored JSON value reopens as formatted text (the editor) and a monetary
/// amount reopens with its declared precision.
#[test]
fn stored_values_render_in_their_widget_shape() {
    use crate::api::{DynField, DynMeta};
    in_owner(|| {
        let meta = DynMeta {
            name: "Shapes".to_string(),
            fields: vec![
                DynField {
                    fieldname: "meta".to_string(),
                    fieldtype: "JSON".to_string(),
                    ..Default::default()
                },
                DynField {
                    fieldname: "amount".to_string(),
                    fieldtype: "Currency".to_string(),
                    precision: Some(2),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let doc = serde_json::json!({ "meta": { "a": 1 }, "amount": 10 });
        let fields = build_form_fields(&meta, &doc, false);

        let json = fields
            .iter()
            .find(|f| f.fieldname == "meta")
            .expect("the JSON field is in the form");
        let text = json.value.get_untracked();
        assert!(
            serde_json::from_str::<Value>(&text).is_ok(),
            "the editor must reopen with parseable JSON, got {text:?}"
        );

        let amount = fields
            .iter()
            .find(|f| f.fieldname == "amount")
            .expect("the Currency field is in the form");
        assert_eq!(amount.value.get_untracked(), "10.00");
    });
}

/// An `Int` travels as a JSON integer. Parsing it as `f64` wrote `7.0`, which
/// the engine rejected with "must be an integer" and failed the whole save.
#[test]
fn an_int_field_is_sent_as_an_integer() {
    in_owner(|| {
        let payload = build_form_payload(&[field("count", "Int", "7")]);
        let value = payload.get("count").expect("the field is in the payload");
        assert!(value.is_i64(), "expected an integer, got {value}");
        assert_eq!(value.as_i64(), Some(7));

        let float = build_form_payload(&[field("ratio", "Float", "2.5")]);
        assert_eq!(
            float.get("ratio").and_then(Value::as_f64),
            Some(2.5),
            "Float keeps its decimals"
        );
    });
}

/// A DocType that starts with a `Tab Break` gets one tab, not two.
///
/// The `new` form leads with a synthetic `ID` field. That field used to open
/// an implicit `""` tab, so the declared first tab was duplicated ("General",
/// "General") and every real field lived in the hidden panel: the form showed
/// the ID and nothing else. Fields that precede the first `Tab Break` belong
/// to it.
#[test]
fn fields_before_the_first_tab_break_join_that_tab() {
    in_owner(|| {
        let mut id = field("name", "Data", "");
        id.is_name = true;
        let fields = vec![
            id,
            field("General", "Tab Break", ""),
            field("Resumen", "Section Break", ""),
            field("subject", "Data", ""),
        ];

        let (tabs, default_tab, sections) = form_layout(&fields);
        assert_eq!(tabs, vec![("General".to_string(), "General".to_string())]);
        assert_eq!(default_tab, "General");
        assert!(
            sections.iter().all(|section| section.tab == "General"),
            "no section may sit in an implicit tab"
        );
        assert_eq!(sections.len(), 2, "ID section plus the declared section");
        assert!(matches!(sections[1].items[0], FormSectionItem::Field(3)));
    });
}

/// the valueless widgets (`Button`, `Attachment Gallery`,
/// `Heading`) carry no editable value — neither the payload nor validation may
/// touch them. Without the skip a required `Button` would block the save with
/// "Campo obligatorio" on a field the user cannot fill.
#[test]
fn valueless_widgets_are_skipped_by_validation_and_payload() {
    in_owner(|| {
        let mut button = field("accion", "Button", "");
        button.reqd = true;
        button.options = Some("mi_modulo.mi_metodo".to_string());
        let mut widget = field("tablero", "Widget", "");
        widget.reqd = true;
        widget.options = Some("mi-app/tablero".to_string());
        let fields = vec![
            button,
            field("galeria", "Attachment Gallery", ""),
            field("titulo", "Heading", "Encabezado"),
            widget,
            field("nota", "Data", "valor"),
        ];

        assert!(validate_form(&fields));
        let payload = build_form_payload(&fields);
        let Value::Object(map) = payload else {
            panic!("the payload must be an object");
        };
        assert_eq!(map.len(), 1, "only the Data field carries a value");
        assert!(map.contains_key("nota"));
    });
}

/// `Table MultiSelect` is the one list policy (see
/// `parse_name_list`) — the wire value is the JSON array, a malformed array is
/// rejected by validation instead of being sent as one garbage element, and a
/// bare legacy name degrades to a one-element array rather than vanishing.
#[test]
fn table_multiselect_validates_and_serializes_as_a_json_name_array() {
    in_owner(|| {
        let ok = field(
            "etiquetas",
            "Table MultiSelect",
            r#"["urgente","revisado"]"#,
        );
        assert!(validate_form(std::slice::from_ref(&ok)));
        let Value::Object(map) = build_form_payload(std::slice::from_ref(&ok)) else {
            panic!("the payload must be an object");
        };
        assert_eq!(
            map.get("etiquetas"),
            Some(&Value::Array(vec![
                Value::String("urgente".to_string()),
                Value::String("revisado".to_string()),
            ]))
        );

        let bad = field("etiquetas", "Table MultiSelect", r#"["a",1]"#);
        assert!(!validate_form(std::slice::from_ref(&bad)));
        assert!(bad.error.get().is_some());

        let bare = field("etiquetas", "Table MultiSelect", "LEGACY-1");
        let Value::Object(map) = build_form_payload(std::slice::from_ref(&bare)) else {
            panic!("the payload must be an object");
        };
        assert_eq!(
            map.get("etiquetas"),
            Some(&Value::Array(vec![Value::String("LEGACY-1".to_string())]))
        );
    });
}

/// A child table used to be dropped from the payload, so editing a parent and
/// saving erased every row. The field now carries the array as JSON and the
/// payload restores it (including the empty case, which must reach the engine
/// so "delete every row" is not a no-op).
#[test]
fn a_child_table_round_trips_through_the_form_payload() {
    use crate::api::{DynField, DynMeta};

    let meta = DynMeta {
        name: "Parent".to_string(),
        fields: vec![DynField {
            fieldname: "lines".to_string(),
            fieldtype: "Table".to_string(),
            options: Some("Line".to_string()),
            ..DynField::default()
        }],
        ..DynMeta::default()
    };
    in_owner(|| {
        let document = serde_json::json!({
            "lines": [{"label": "a", "amount": 2}],
        });
        let fields = build_form_fields(&meta, &document, false);
        assert_eq!(
            build_form_payload(&fields).get("lines"),
            Some(&serde_json::json!([{"label": "a", "amount": 2}]))
        );

        let empty = build_form_fields(&meta, &serde_json::json!({}), false);
        assert_eq!(
            build_form_payload(&empty).get("lines"),
            Some(&serde_json::json!([]))
        );
    });
}

/// A stored child-table value that is not a JSON array belongs to the document,
/// not to the form. Rewriting it to `[]` (the old behaviour) meant opening such
/// a document and pressing Save erased rows the user never touched, so the
/// field keeps the raw value and the payload omits the field instead.
#[test]
fn a_malformed_child_table_is_preserved_and_never_sent_as_empty() {
    use crate::api::{DynField, DynMeta};

    let meta = DynMeta {
        name: "Parent".to_string(),
        fields: vec![DynField {
            fieldname: "lines".to_string(),
            fieldtype: "Table".to_string(),
            options: Some("Line".to_string()),
            ..DynField::default()
        }],
        ..DynMeta::default()
    };
    in_owner(|| {
        let document = serde_json::json!({ "lines": "not-a-list" });
        let fields = build_form_fields(&meta, &document, false);
        assert_eq!(fields[0].value.get_untracked(), "not-a-list");

        let payload = build_form_payload(&fields);
        assert!(
            payload.get("lines").is_none(),
            "a malformed child table must be omitted, not sent as []: {payload}"
        );
    });
}
