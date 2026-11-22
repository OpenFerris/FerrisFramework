//! The field types the runtime field creator offers, and one hint per type.
//!
//! Split from the dialog that renders the picker so the curated list, the
//! explanation each choice carries and the coverage test fit in one file: a
//! fieldtype offered without a hint is the failure mode this table exists to
//! prevent (the `<select>` is narrow, the caption under it is what the user
//! actually reads).

/// The field types the add-field form offers.
///
/// Curated from the canonical vocabulary — the same list the write endpoint
/// validates against, the renderer dispatches on and the WIT/TS generators
/// are keyed on: `ferris_schema::fieldtype::FIELDTYPES`, which is always
/// compiled (no `native` feature). Anything listed here therefore cannot be
/// rejected as an unknown fieldtype after the user submits it.
///
/// The order is Frappe's own (the documentation's fieldtype page), because it
/// groups the choices the way a customizer thinks: text, then links, then
/// numbers, then dates, then the rich/structured types.
///
/// Deliberately absent, with the reason — this is a *field creator* for
/// ordinary forms, not the full registry:
/// - `Heading`: a caption, not a field; the editor has no use for one yet.
/// - `Table`/`Table MultiSelect`: need a child `DocType` to point at, and the
///   Desk edits child tables in their own view.
///
/// The three **breaks are offered on purpose**: they are how a form is
/// organised into tabs, sections and columns, and a customizer that cannot add
/// a section cannot lay out a form. Their `fieldname` is the identity (for a
/// `Tab Break` it is the tab value) and their `label` is what the form shows.
/// - `HTML`: raw markup, not a user value (render-only).
/// - `Read Only`, `Button`: they render an existing value or an action
///   rather than let one be typed, so they add nothing here.
/// - `Widget` **is** offered: it is how a form reserves a place for a widget
///   (`options` = `<app>/<widget>`), which is a layout decision the editor owns
///   even though the field carries no value.
///
/// `Dynamic Link`, `Week`, `Markdown Editor`, `Duration`, `Rating`,
/// `Geolocation`, `Signature`, `Barcode` and `Attachment Gallery` are
/// included: they are typed, editable values the renderer has a widget for.
pub const FIELD_TYPES: &[&str] = &[
    "Data",
    "Text",
    "Small Text",
    "Long Text",
    "Text Editor",
    "Markdown Editor",
    "Code",
    "Link",
    "Dynamic Link",
    "Select",
    "Int",
    "Float",
    "Currency",
    "Percent",
    "Rating",
    "Duration",
    "Check",
    "Date",
    "Datetime",
    "Time",
    "Week",
    "Password",
    "Secret",
    "Color",
    "Email",
    "Phone",
    "URL",
    "JSON",
    "Geolocation",
    "Signature",
    "Barcode",
    "Image",
    "File",
    "Attach",
    "Attach Image",
    "Attachment Gallery",
    "Widget",
    "Section Break",
    "Tab Break",
    "Column Break",
];

/// One-line hint per offered fieldtype, shown next to the picker so the
/// customizer knows what the type expects — above all what belongs in
/// `options`, which is the field this dialog leaves free-form and the one
/// place a wrong guess costs a round trip to the server.
///
/// [`hint`] falls back to an empty string, but the `every_offered_fieldtype_has_a_hint`
/// test refuses to let an entry of [`FIELD_TYPES`] ship without one, so the
/// table cannot silently fall behind the catalog.
///
/// A hint repeats the type's name deliberately: the `<select>` is narrow, and
/// the caption under it is where the explanation is actually read.
const HINTS: &[(&str, &str)] = &[
    ("Data", "texto corto de una línea (el tipo por defecto)"),
    ("Text", "varias líneas de texto sin formato"),
    ("Small Text", "como Text, para notas de longitud media"),
    ("Long Text", "texto largo; no aparece en listas ni filtros"),
    ("Text Editor", "editor WYSIWYG (Quill), guarda HTML"),
    (
        "Markdown Editor",
        "markdown con vista previa en el formulario",
    ),
    ("Code", "fragmento de código en un área monoespaciada"),
    (
        "Link",
        "referencia a otra ficha; `options` = el nombre del DocType",
    ),
    (
        "Dynamic Link",
        "referencia a un DocType que decide otro campo; `options` = el campo que lo indica",
    ),
    (
        "Select",
        "lista de opciones; `options` = los valores, uno por línea",
    ),
    ("Int", "número entero"),
    ("Float", "número con decimales"),
    ("Currency", "importe; `options` = la moneda por defecto"),
    ("Percent", "porcentaje"),
    ("Rating", "puntuación de 0 a 5 estrellas"),
    ("Duration", "duración en segundos"),
    ("Check", "casilla de sí/no"),
    ("Date", "fecha"),
    ("Datetime", "fecha y hora"),
    ("Time", "hora"),
    ("Week", "semana del año"),
    ("Password", "valor oculto, nunca se muestra en claro"),
    ("Secret", "secreto cifrado; el Desk solo ve la máscara"),
    ("Color", "selector de color"),
    ("Email", "dirección de correo validada"),
    ("Phone", "teléfono validado"),
    ("URL", "dirección web validada"),
    ("JSON", "objeto o array JSON"),
    ("Geolocation", "punto o forma sobre un mapa (GeoJSON)"),
    ("Signature", "firma dibujada; se guarda como imagen"),
    (
        "Barcode",
        "código de barras; `options` = el symbology (por defecto CODE128)",
    ),
    ("Image", "imagen"),
    ("File", "fichero adjunto"),
    ("Attach", "fichero adjunto (mismo almacén que File)"),
    ("Attach Image", "fichero, restringido a imágenes"),
    (
        "Attachment Gallery",
        "galería de ficheros; `options` = el campo con las rutas",
    ),
    (
        "Widget",
        "widget dentro del formulario; `options` = `<app>/<widget>` (sin valor propio)",
    ),
    (
        "Section Break",
        "sección nueva; la etiqueta es el título de la sección",
    ),
    (
        "Tab Break",
        "pestaña nueva; el nombre es el valor de la pestaña y la etiqueta lo que se ve",
    ),
    ("Column Break", "columna nueva dentro de la sección actual"),
];

/// The hint for `fieldtype`, or `""` for a name the table does not know —
/// which the coverage test turns into a failure rather than a silent blank.
pub fn hint(fieldtype: &str) -> &'static str {
    HINTS
        .iter()
        .find(|(name, _)| *name == fieldtype)
        .map_or("", |(_, hint)| *hint)
}

/// Trimmed value as `Some`, or `None` when empty.
pub fn nonempty(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{FIELD_TYPES, HINTS, hint};
    use ferris_schema::fieldtype::{FIELDTYPES, canonical};

    /// The catalog is a promise to the user: every name it offers must be one
    /// the write endpoint accepts. `add_custom_field` answers `InvalidFieldType`
    /// for anything outside [`FIELDTYPES`], so a typo here would surface as a
    /// server refusal *after* the form was filled in — this test is what keeps
    /// that from shipping.
    #[test]
    fn every_offered_fieldtype_is_canonical() {
        for name in FIELD_TYPES {
            assert!(
                FIELDTYPES.contains(name),
                "{name} is offered but is not a canonical fieldtype"
            );
            assert_eq!(
                canonical(name),
                Some(*name),
                "{name} is not canonical spelling"
            );
        }
    }

    /// The list is a curated subset, so it must at least be it: a field type
    /// that vanishes from the vocabulary must not keep being offered.
    #[test]
    fn the_catalog_has_no_duplicates() {
        let mut seen = std::collections::HashSet::new();
        for name in FIELD_TYPES {
            assert!(seen.insert(name), "{name} is listed twice");
        }
    }

    /// Every offered type carries a hint, so a new entry of [`FIELD_TYPES`]
    /// cannot ship with a blank caption — the picker explains `options` for
    /// the types that need it.
    #[test]
    fn every_offered_fieldtype_has_a_hint() {
        for name in FIELD_TYPES {
            assert!(!hint(name).is_empty(), "{name} is offered without a hint");
        }
    }

    /// The hint table has no orphans: a hint for a type the picker does not
    /// offer would never be read, and is a sign the two drifted.
    #[test]
    fn the_hints_name_only_offered_fieldtypes() {
        for (name, _) in HINTS {
            assert!(
                FIELD_TYPES.contains(name),
                "{name} has a hint but is not offered"
            );
        }
    }
}
