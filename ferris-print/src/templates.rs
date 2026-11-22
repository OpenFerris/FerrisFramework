//! Built-in Typst layouts shipped with the framework.
//!
//! These are seeds, not a second format registry: a `Print Format` row is the
//! unit the engine renders. `ferris generate templates` writes these files to a
//! project so they can be copied into a row and edited.

/// The generic layout used when a `DocType` has no `Print Format`: a two-column
/// table of the document's fields.
///
/// It reads `_ferris_data` (the wrapper the engine synthesises) instead of
/// top-level variables, so it works for any document shape.
pub const GENERIC_TEMPLATE: &str = r#"#set text(size: 9pt)
#table(
  columns: (auto, 1fr),
  stroke: 0.5pt,
  .._ferris_data.pairs().map(((key, value)) => (
    text(weight: "bold")[#key],
    repr(value),
  )).flatten(),
)
"#;

/// Built-in print templates, as `(file stem, readable name, Typst source)`.
/// Used by `ferris generate templates` and by the benchmark.
pub const BUILTIN_TEMPLATES: &[(&str, &str, &str)] = &[
    ("invoice", "Invoice", include_str!("templates/invoice.typ")),
    (
        "purchase_order",
        "Purchase Order",
        include_str!("templates/purchase_order.typ"),
    ),
    (
        "standard_letter",
        "Standard Letter",
        include_str!("templates/standard_letter.typ"),
    ),
    (
        "delivery_note",
        "Delivery Note",
        include_str!("templates/delivery_note.typ"),
    ),
    (
        "payment_receipt",
        "Payment Receipt",
        include_str!("templates/payment_receipt.typ"),
    ),
    (
        "salary_slip",
        "Salary Slip",
        include_str!("templates/salary_slip.typ"),
    ),
];
