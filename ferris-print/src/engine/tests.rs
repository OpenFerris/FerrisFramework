use super::export::export;
use super::options::ResolvedPrint;
use super::world::{TypstResources, TypstWorld};
use super::*;

use ferris_core::Value;
use std::collections::HashMap;
use std::sync::Arc;
use typst::foundations::Bytes;

#[test]
fn test_print_config() {
    let config = PrintConfig::default();
    assert_eq!(config.limits.max_template_bytes, 256 * 1024);
    assert_eq!(config.limits.max_output_bytes, 4 * 1024 * 1024);
}

#[test]
fn test_print_format() {
    let format = PrintFormatDef::new("test".to_string(), "Test".to_string(), "#hello".to_string());
    assert_eq!(format.name, "test");
    assert_eq!(format.engine, FormatEngine::Typst);
    assert!(format.enabled);
}

#[test]
fn test_paper_size() {
    let a4 = PaperSize::A4;
    let (w, h) = a4.dimensions();
    assert!((w - 210.0).abs() < f32::EPSILON);
    assert!((h - 297.0).abs() < f32::EPSILON);
}

#[test]
fn paper_and_orientation_parse_the_stored_spelling() {
    assert_eq!("Letter".parse::<PaperSize>().unwrap(), PaperSize::Letter);
    assert_eq!(
        "landscape".parse::<Orientation>().unwrap(),
        Orientation::Landscape
    );
    assert!("A9".parse::<PaperSize>().is_err());
    assert!("sideways".parse::<Orientation>().is_err());
}

#[test]
fn engines_parse_the_current_and_historical_spellings() {
    assert_eq!(
        "jinja".parse::<FormatEngine>().unwrap(),
        FormatEngine::Jinja
    );
    assert_eq!(
        "Server".parse::<FormatEngine>().unwrap(),
        FormatEngine::Typst
    );
    assert!("js".parse::<FormatEngine>().is_err());
}

#[test]
fn test_is_valid_identifier() {
    assert!(TypstWorld::is_valid_identifier("hello"));
    assert!(TypstWorld::is_valid_identifier("customer-name_2"));
    assert!(!TypstWorld::is_valid_identifier("2foo"));
    assert!(!TypstWorld::is_valid_identifier("a b"));
    assert!(!TypstWorld::is_valid_identifier(""));
    assert!(!TypstWorld::is_valid_identifier("fóo"));
}

#[test]
fn test_synthesize_source() {
    let mut map = HashMap::new();
    map.insert("name".to_string(), Value::string("Ferris"));
    let data = Value::object(map);

    let source = TypstWorld::synthesize_source("#name", &data, "");
    assert!(source.contains("#let _ferris_data = json.decode("));
    assert!(source.contains("#let name = _ferris_data.at(\"name\")"));
    assert!(source.trim_end().ends_with("#name"));
}

#[test]
fn test_resources_load() {
    let resources = TypstResources::new(None).unwrap();
    assert!(!resources.fonts.is_empty(), "bundled fonts must load");
    assert!(resources.book.families().count() > 0);
}

fn render_world() -> (TypstWorld, Value) {
    let resources = Arc::new(TypstResources::new(None).unwrap());
    let mut map = HashMap::new();
    map.insert("customer".to_string(), Value::string("Ferris"));
    let data = Value::object(map);
    let json = serde_json::to_vec(&data).unwrap();
    let template = "#set page(width: 80mm, height: 40mm)\n#set text(size: 12pt)\nHello, #customer!";
    let world = TypstWorld::new(resources, None, template, &data, Bytes::new(json), "");
    (world, data)
}

#[test]
fn test_render_pdf() {
    let (world, _data) = render_world();
    let pdf = export(&world, PrintFormat::Pdf, 1.0).unwrap();
    assert!(!pdf.is_empty());
    assert!(pdf.starts_with(b"%PDF"));
}

#[test]
fn test_render_svg() {
    let (world, _data) = render_world();
    let svg = String::from_utf8(export(&world, PrintFormat::Svg, 1.0).unwrap()).unwrap();
    assert!(svg.starts_with("<svg"));
}

#[test]
fn test_render_png() {
    let (world, _data) = render_world();
    let png = export(&world, PrintFormat::Png, 1.0).unwrap();
    assert!(png.starts_with(b"\x89PNG"));
}

#[test]
fn test_render_html() {
    let resources = Arc::new(TypstResources::new(None).unwrap());
    let mut map = HashMap::new();
    map.insert("customer".to_string(), Value::string("Ferris"));
    let data = Value::object(map);
    let json = serde_json::to_vec(&data).unwrap();
    let template = "#emph[Hello, #customer!]";
    let world = TypstWorld::new(resources, None, template, &data, Bytes::new(json), "");
    let html = String::from_utf8(export(&world, PrintFormat::Html, 1.0).unwrap()).unwrap();
    assert!(html.contains("<!DOCTYPE html>"));
    assert!(html.contains("Ferris"));
}

#[test]
fn test_render_missing_variable_errors() {
    let resources = Arc::new(TypstResources::new(None).unwrap());
    let data = Value::object(HashMap::new());
    let json = serde_json::to_vec(&data).unwrap();
    let world = TypstWorld::new(
        resources,
        None,
        "#missing_variable",
        &data,
        Bytes::new(json),
        "",
    );
    let err = export(&world, PrintFormat::Pdf, 1.0).unwrap_err();
    assert!(err.to_string().contains("missing_variable"), "got: {err}");
}

fn test_engine() -> PrintEngine {
    PrintEngine::new(PrintConfig {
        font_dir: None,
        template_dir: None,
        ..Default::default()
    })
    .expect("bundled fonts load")
}

fn test_format(template: &str) -> PrintFormatDef {
    PrintFormatDef::new("test".to_string(), "Doc".to_string(), template.to_string())
}

fn sample_data() -> Value {
    let mut map = HashMap::new();
    map.insert("name".to_string(), Value::string("Ferris"));
    Value::object(map)
}

#[test]
fn render_format_applies_page_options_and_returns_a_pdf() {
    let options = PrintOptions {
        paper_size: Some(PaperSize::A3),
        orientation: Some(Orientation::Landscape),
        margins: Some(Margins {
            top: 5.0,
            bottom: 5.0,
            left: 8.0,
            right: 8.0,
        }),
        scale: Some(0.9),
        watermark: Some("DRAFT".to_string()),
        ..Default::default()
    };
    let pdf = test_engine()
        .render_format(&test_format("Hello #name"), &sample_data(), &options)
        .expect("PDF renders with page options");
    assert!(pdf.starts_with(b"%PDF"));
    assert!(pdf.len() > 100);
}

#[test]
fn a_zero_or_negative_scale_is_a_validation_error() {
    let err = ResolvedPrint::resolve(
        &test_format("x"),
        &PrintOptions {
            scale: Some(0.0),
            ..Default::default()
        },
    )
    .expect_err("scale 0 is not renderable");
    assert!(err.to_string().contains("scale"), "{err}");
}

#[test]
fn html_rejects_a_content_scale_instead_of_ignoring_it() {
    let err = ResolvedPrint::resolve(
        &test_format("x"),
        &PrintOptions {
            format: PrintFormat::Html,
            scale: Some(1.5),
            ..Default::default()
        },
    )
    .expect_err("HTML is not paged");
    assert!(err.to_string().contains("scale"), "{err}");
}

#[test]
fn the_page_prelude_carries_paper_margins_orientation_and_watermark() {
    let resolved = ResolvedPrint::resolve(
        &test_format("x"),
        &PrintOptions {
            paper_size: Some(PaperSize::A3),
            orientation: Some(Orientation::Landscape),
            margins: Some(Margins {
                top: 5.0,
                bottom: 6.0,
                left: 7.0,
                right: 8.0,
            }),
            watermark: Some("A\"B".to_string()),
            ..Default::default()
        },
    )
    .expect("options resolve");
    let prelude = resolved.page_prelude();
    assert!(prelude.contains("paper: \"a3\""), "{prelude}");
    assert!(prelude.contains("flipped: true"), "{prelude}");
    assert!(prelude.contains("top: 5mm"), "{prelude}");
    assert!(prelude.contains("right: 8mm"), "{prelude}");
    assert!(prelude.contains("A\\\"B"), "{prelude}");
}

/// The generic layout the API prints when a `DocType` has no `Print Format` must
/// render in **both** targets: PDF is what the Desk asks for, and HTML is an
/// advertised `format` — Typst rejects a page configuration in the HTML target
/// ("page configuration is not allowed inside of containers"), which used to
/// fail every HTML print.
#[test]
fn the_generic_layout_renders_in_html_and_pdf() {
    let engine = test_engine();
    let mut map = HashMap::new();
    map.insert("label".to_string(), Value::string("Widgets"));
    map.insert("owner".to_string(), Value::string("admin@local"));
    let data = Value::object(map);
    let format = PrintFormatDef::new(
        "generic".to_string(),
        "Widgets".to_string(),
        crate::GENERIC_TEMPLATE.to_string(),
    );

    let html = engine
        .render_format(
            &format,
            &data,
            &PrintOptions {
                format: PrintFormat::Html,
                ..Default::default()
            },
        )
        .expect("HTML renders the generic layout");
    let html = String::from_utf8(html).expect("utf-8");
    assert!(html.contains("Widgets"), "{html}");

    let pdf = engine
        .render_format(&format, &data, &PrintOptions::default())
        .expect("PDF renders the generic layout");
    assert!(pdf.starts_with(b"%PDF"));
}

fn invoice_data() -> Value {
    Value::from_json(serde_json::json!({
        "number": "INV-7",
        "customer": "#1 *ACME*",
        "items": [
            { "name": "Widget", "qty": 2, "rate": 12.5 },
            { "name": "Bolt", "qty": 10, "rate": 0.25 }
        ]
    }))
}

const JINJA_INVOICE: &str = r"#set page(width: 120mm, height: 90mm)
#set text(size: 9pt)
#text(size: 15pt)[Invoice {{ doc.number | typst }}]
{% for item in doc.items %}
#text[{{ item.name | typst }}] — {{ item.qty }} × {{ item.rate | money }}
{% endfor %}
#text(size: 12pt)[Total: {{ params.total | money }}]
";

#[test]
fn a_jinja_format_renders_a_pdf_and_formats_its_values() {
    let mut format = PrintFormatDef::new_jinja(
        "invoice".to_string(),
        "Sales Invoice".to_string(),
        JINJA_INVOICE.to_string(),
    );
    format.parameters.insert(
        "total".to_string(),
        Value::from_json(serde_json::json!(27.5)),
    );

    let pdf = test_engine()
        .render_format(&format, &invoice_data(), &PrintOptions::default())
        .expect("Jinja → Typst → PDF");
    assert!(pdf.starts_with(b"%PDF"));
    assert!(pdf.len() > 100);
}

#[test]
fn request_parameters_override_the_format_defaults() {
    let mut format = PrintFormatDef::new_jinja(
        "param".to_string(),
        "Doc".to_string(),
        "Value: {{ params.value }}".to_string(),
    );
    format
        .parameters
        .insert("value".to_string(), Value::string("default"));
    let options = PrintOptions {
        params: HashMap::from([("value".to_string(), Value::string("request"))]),
        ..Default::default()
    };
    let source = test_engine()
        .render_source(&format, &sample_data(), &options)
        .expect("Jinja renders");
    assert!(source.contains("Value: request"), "{source}");
}

#[test]
fn a_document_value_cannot_inject_typst_syntax() {
    let format = PrintFormatDef::new_jinja(
        "escape".to_string(),
        "Doc".to_string(),
        "#text[{{ doc.name | typst }}]".to_string(),
    );
    let mut map = HashMap::new();
    map.insert("name".to_string(), Value::string("#1 *bold* [x]"));
    let data = Value::object(map);
    let source = test_engine()
        .render_source(&format, &data, &PrintOptions::default())
        .expect("Jinja renders");
    assert!(source.contains("\\#1 \\*bold\\* \\[x\\]"), "{source}");
    let pdf = test_engine()
        .render_format(&format, &data, &PrintOptions::default())
        .expect("escaped Typst compiles");
    assert!(pdf.starts_with(b"%PDF"));
}

#[test]
fn typst_value_exposes_a_json_value_as_code() {
    let format = PrintFormatDef::new_jinja(
        "literal".to_string(),
        "Doc".to_string(),
        "#let rows = {{ doc.items | typst_value }}\n#rows.len()".to_string(),
    );
    let source = test_engine()
        .render_source(&format, &invoice_data(), &PrintOptions::default())
        .expect("Jinja renders");
    assert!(source.contains("#let rows = ("), "{source}");
    assert!(source.contains("name: \"Widget\""), "{source}");
}

#[test]
fn a_template_over_the_limit_is_rejected_before_compiling() {
    let engine = PrintEngine::new(PrintConfig {
        font_dir: None,
        template_dir: None,
        limits: TemplateLimits {
            max_template_bytes: 8,
            ..TemplateLimits::default()
        },
    })
    .expect("engine");
    let err = engine
        .render_format(
            &PrintFormatDef::new_jinja(
                "big".to_string(),
                "Doc".to_string(),
                "this template is longer than eight bytes".to_string(),
            ),
            &sample_data(),
            &PrintOptions::default(),
        )
        .expect_err("the template is over the limit");
    assert!(err.to_string().contains("template limit"), "{err}");
}

#[test]
fn a_jinja_syntax_error_is_a_validation_error() {
    let err = test_engine()
        .render_format(
            &PrintFormatDef::new_jinja(
                "broken".to_string(),
                "Doc".to_string(),
                "{% if %}".to_string(),
            ),
            &sample_data(),
            &PrintOptions::default(),
        )
        .expect_err("the template does not parse");
    assert!(err.to_string().contains("failed to render"), "{err}");
}

#[test]
fn a_jinja_format_can_render_html() {
    let format = PrintFormatDef::new_jinja(
        "html".to_string(),
        "Doc".to_string(),
        "#emph[Hello, {{ doc.name | typst }}!]".to_string(),
    );
    let html = test_engine()
        .render_format(
            &format,
            &sample_data(),
            &PrintOptions {
                format: PrintFormat::Html,
                ..Default::default()
            },
        )
        .expect("HTML renders");
    let html = String::from_utf8(html).expect("utf-8");
    assert!(html.contains("Hello, Ferris!"), "{html}");
}

fn template_fixture(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("ferris-print-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("template fixture dir");
    dir
}

#[test]
fn jinja_includes_and_extends_resolve_inside_the_format_folder() {
    let root = template_fixture("jinja");
    let format_dir = root.join("invoice");
    std::fs::create_dir_all(&format_dir).expect("format dir");
    std::fs::write(
        format_dir.join("base.typ"),
        "#set text(size: 11pt)\n{% block body %}{% endblock %}",
    )
    .expect("base");
    std::fs::write(format_dir.join("line.typ"), "Line: {{ doc.name | typst }}").expect("partial");

    let engine = PrintEngine::new(PrintConfig {
        font_dir: None,
        template_dir: Some(root.to_string_lossy().into_owned()),
        ..Default::default()
    })
    .expect("engine");
    let mut format = PrintFormatDef::new_jinja(
        "inherited".to_string(),
        "Doc".to_string(),
        "{% extends \"base.typ\" %}{% block body %}{% include \"line.typ\" %}{% endblock %}"
            .to_string(),
    );
    format.template_dir = Some("invoice".to_string());

    let source = engine
        .render_source(&format, &sample_data(), &PrintOptions::default())
        .expect("include + extends render");
    assert!(source.contains("Line: Ferris"), "{source}");

    let pdf = engine
        .render_format(&format, &sample_data(), &PrintOptions::default())
        .expect("the inherited template compiles");
    assert!(pdf.starts_with(b"%PDF"));

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_template_folder_cannot_escape_the_root() {
    let root = template_fixture("escape");
    let engine = PrintEngine::new(PrintConfig {
        font_dir: None,
        template_dir: Some(root.to_string_lossy().into_owned()),
        ..Default::default()
    })
    .expect("engine");
    let mut format =
        PrintFormatDef::new_jinja("escape".to_string(), "Doc".to_string(), "text".to_string());
    format.template_dir = Some("../secret".to_string());
    let error = engine
        .render_source(&format, &sample_data(), &PrintOptions::default())
        .expect_err("traversal is refused");
    assert!(
        error.to_string().contains("unsafe template folder"),
        "{error}"
    );
    let _ = std::fs::remove_dir_all(&root);
}
