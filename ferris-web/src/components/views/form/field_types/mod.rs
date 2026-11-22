//! Form view field type components.
//!
//! One module per family of inputs — text, number, choice, date/time, contact,
//! colour, link and the miscellaneous rest — each re-exported here so the
//! renderer's `field_types::*` import and the historical component names keep
//! resolving. A new `fieldtype` adds a component to the family it belongs to
//! and a match arm in `field_renderer`, never a new branch of a catch-all.

mod button;
mod choice;
mod color;
mod contact;
mod datetime;
mod doc_context;
mod editor;
mod gallery;
mod geo;
mod link;
mod misc;
mod number;
mod signature;
mod text;
mod widget;

pub use button::ButtonField;
pub use choice::{CheckField, RatingField, SelectField};
pub use color::ColorField;
pub use contact::{EmailField, PhoneField, UrlField};
pub use datetime::{DateField, DateTimeField, TimeField, WeekField};
pub use doc_context::FormDocContext;
pub use editor::{MarkdownField, TextEditorField};
pub use gallery::AttachmentGalleryField;
pub use geo::GeolocationField;
pub use link::{LinkFieldComponent, MultiLinkField, TagsField};
pub use misc::{
    AttachField, BarcodeField, HtmlBlockField, JsonField, PasswordField, ReadOnlyField,
    TableNoticeField,
};
pub use number::{CurrencyField, NumberField, PercentField};
pub use signature::SignatureField;
pub use text::{CodeField, TextField};
pub use widget::WidgetField;
