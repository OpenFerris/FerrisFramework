//! Desk view components

pub mod calendar_view;
pub mod dashboard;
pub mod dashboard_doc_view;
pub mod doc_route;
pub mod form_view;
pub mod gantt_view;
pub mod home;
pub mod kanban_view;
pub mod list_view;
pub mod plugin_page;
pub mod report_view;
pub mod services;
pub mod settings_permissions;
pub mod settings_security;
pub mod settings_view;
pub mod tree_view;
pub mod views_components;
pub mod widget_host;

pub use calendar_view::CalendarView;
pub use dashboard::DashboardGrid;
pub use dashboard_doc_view::DashboardDocView;
pub use doc_route::DocRoute;
pub use form_view::FormView;
pub use gantt_view::GanttView;
pub use home::HomeView;
pub use kanban_view::KanbanView;
pub use list_view::ListView;
pub use plugin_page::PluginPage;
pub use report_view::ReportView;
pub use settings_view::SettingsView;
pub use tree_view::TreeView;
