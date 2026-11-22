//! Tests for the integration DocType declarations.

use super::*;
use ferris_schema::get_doctype;

#[test]
fn every_doctype_registers_with_fields() {
    register_doctypes();
    for meta in doctype_metas() {
        let got = get_doctype(&meta.name).expect("registered");
        assert_eq!(got.module, MODULE, "{}", meta.name);
        assert!(!got.fields.is_empty(), "{} needs fields", meta.name);
    }
}

#[test]
fn exactly_google_and_ldap_settings_are_singles() {
    register_doctypes();
    let singles: Vec<_> = doctype_metas()
        .into_iter()
        .filter(|m| m.is_single)
        .map(|m| m.name)
        .collect();
    assert_eq!(singles, vec!["Google Settings", "LDAP Settings"]);
}

#[test]
fn register_is_idempotent() {
    register_doctypes();
    register_doctypes();
    assert!(get_doctype("Connected App").is_some());
    assert!(get_doctype("LDAP Settings").is_some());
}

/// The credentials the runtime has to *use* are recoverable `Secret` fields,
/// not one-way `Password` ones (H3). The exact set is pinned here so a new
/// integration cannot reintroduce the bug by declaring a `client_secret` as
/// `Password` and silently reading the mask back.
#[test]
fn the_integration_credentials_are_recoverable_secrets() {
    register_doctypes();
    let expected = [
        ("Connected App", "client_secret"),
        ("Webhook", "webhook_secret"),
        ("Google Settings", "client_secret"),
        ("Google Settings", "refresh_token"),
        ("Social Login Key", "client_secret"),
        ("LDAP Settings", "bind_password"),
        ("OAuth Client", "client_secret"),
        ("OAuth Provider", "client_secret"),
    ];
    for (doctype, fieldname) in expected {
        let meta = get_doctype(doctype).expect("registered");
        let field = meta
            .fields
            .iter()
            .find(|field| field.fieldname == fieldname)
            .unwrap_or_else(|| panic!("{doctype}.{fieldname} is declared"));
        assert_eq!(field.fieldtype, "Secret", "{doctype}.{fieldname}");
        // A secret can never be a list column or a lookup key: the stored
        // value is ciphertext, and the mask is all a read returns.
        assert!(!field.in_list_view, "{doctype}.{fieldname}");
        assert!(!field.unique, "{doctype}.{fieldname}");
        assert!(!field.search_index, "{doctype}.{fieldname}");
        assert!(!field.in_standard_filter, "{doctype}.{fieldname}");
    }
    // The title/search/index declarations must not point at one either.
    for meta in doctype_metas() {
        let secret_names: Vec<&str> = meta
            .fields
            .iter()
            .filter(|field| field.fieldtype == "Secret")
            .map(|field| field.fieldname.as_str())
            .collect();
        for secret in &secret_names {
            assert_ne!(meta.title_field.as_deref(), Some(*secret), "{}", meta.name);
            assert!(!meta.search_fields.iter().any(|field| field == secret));
            assert!(
                !meta
                    .indexes
                    .iter()
                    .any(|index| index.fields.iter().any(|field| field == secret)),
                "{} indexes {secret}",
                meta.name
            );
        }
    }
}
