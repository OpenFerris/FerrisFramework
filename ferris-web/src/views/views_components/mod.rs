//! Reusable Desk building blocks (all composed from `components::ui::*`).

pub mod chart_placeholder;
pub mod collection;
pub mod doctypes;
pub mod page;
pub mod pagination;
pub mod report_filters;
pub mod stat_card;

pub use chart_placeholder::ChartPlaceholder;
pub use collection::use_collection_actions;
pub use doctypes::use_desk_doctypes;
pub use page::{
    EmptyState, Feedback, FeedbackKind, LoadingLines, NotFoundPage, PageHeader, SectionTitle,
};
pub use pagination::Pagination;
pub use report_filters::ReportFilters;
pub use stat_card::{StatCard, StatCardView};
