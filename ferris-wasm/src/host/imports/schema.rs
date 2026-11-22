//! `schema` import bridge: the full `DocType` declaration escape hatch.
//!
//! The descriptor's `doctype` record is a convenient subset; an app that needs
//! a property it does not carry declares the whole `DoctypeMeta` as JSON here.
//! The declaration is validated **now** — canonical fieldtypes, reserved names,
//! cross-app ownership — and staged on the host state. The plugin store drains
//! the staging area after `describe()` returns and registers the doctypes with
//! the schema registry, so a failing declaration aborts the load instead of
//! leaving a half-declared app.

use ferris_schema::DoctypeMeta;

use crate::ferris::framework::schema as wit_schema;
use crate::ferris::framework::types::Error as WitError;
use crate::host::{Capability, HostState};

impl wit_schema::Host for HostState {
    fn declare_doctype(&mut self, definition: String) -> Result<(), WitError> {
        // A declaration is a schema write: `schema` is primordial, but the
        // capability check keeps the seam explicit and future-proof.
        self.require(Capability::Schema)?;
        let meta: DoctypeMeta = serde_json::from_str(&definition).map_err(|error| {
            WitError::ValidationFailed(format!("invalid DocType JSON: {error}"))
        })?;
        ferris_schema::registry::validate_app_doctype(&self.plugin_name, &meta)
            .map_err(|error| WitError::ValidationFailed(error.to_string()))?;
        self.declared_doctypes.push(definition);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ferris::framework::schema::Host as SchemaHost;
    use crate::host::NullHost;
    use ferris_schema::FieldMeta;
    use std::sync::Arc;

    fn state() -> HostState {
        let mut state = HostState::new(Arc::new(NullHost));
        state.set_plugin_name("test-app".to_string());
        state
    }

    fn definition(name: &str, fieldtype: &str) -> String {
        let meta = DoctypeMeta {
            name: name.to_string(),
            module: "test-app".to_string(),
            fields: vec![FieldMeta {
                fieldname: "title".to_string(),
                label: Some("Title".to_string()),
                fieldtype: fieldtype.to_string(),
                reqd: true,
                in_list_view: true,
                ..FieldMeta::default()
            }],
            ..DoctypeMeta::default()
        };
        serde_json::to_string(&meta).expect("the fixture serializes")
    }

    #[test]
    fn a_valid_declaration_is_staged_and_drained_once() {
        let mut state = state();
        state
            .declare_doctype(definition("Parity Note", "Data"))
            .expect("a valid declaration is accepted");
        let declared = state.take_declared_doctypes();
        assert_eq!(declared.len(), 1);
        assert!(declared[0].contains("Parity Note"));
        assert!(state.take_declared_doctypes().is_empty(), "consumed once");
    }

    #[test]
    fn an_unknown_fieldtype_is_refused_before_registration() {
        let mut state = state();
        let error = state
            .declare_doctype(definition("Bad Field", "Sparkle"))
            .expect_err("an unknown fieldtype is refused");
        assert!(
            matches!(&error, WitError::ValidationFailed(message) if message.contains("Sparkle")),
            "unexpected error: {error:?}"
        );
        assert!(state.take_declared_doctypes().is_empty());
    }

    #[test]
    fn a_reserved_framework_doctype_is_refused() {
        let mut state = state();
        let error = state
            .declare_doctype(definition("DocType", "Data"))
            .expect_err("a framework DocType is reserved");
        assert!(matches!(error, WitError::ValidationFailed(_)));
    }
}
