//! View services - business logic separated from UI components.

pub mod view_core;

pub mod form_service;
pub mod list_service;

pub use form_service::{
    FormField, FormSection, FormSectionItem, FormService, build_form_fields, build_form_layout,
    build_form_payload, condition, duplicate_payload, form_layout, parse_name_list, validate_form,
    validate_form_with,
};
pub use list_service::ListService;
