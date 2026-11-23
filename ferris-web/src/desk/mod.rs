//! Desk UI components

pub mod desktop;
pub mod global_search;
pub mod icon;
pub mod language_picker;
pub mod login;
pub mod mobile_nav;
pub mod notifications;
pub mod notifications_panel;
pub mod relogin;
pub mod session;
pub mod sidebar;
pub mod theme_dialog;
pub mod top_bar;
pub mod user_menu;

pub use desktop::Desktop;
pub use global_search::{GlobalSearch, SavedEntry};
pub use icon::DeskGlyph;
pub use language_picker::LanguagePicker;
pub use login::{LoginPage, LoginView};
pub use mobile_nav::MobileNav;
pub use notifications_panel::NotificationsPanel;
pub use session::{
    DeskEvent, DeskIcon, DeskIconPlacement, DeskModule, DeskSearchHit, DeskSidebarEntry, EventFeed,
    Notification, NotificationFeed, NotificationSettings,
};
pub use sidebar::Sidebar;
pub use theme_dialog::ThemeDialog;
pub use top_bar::DeskTopBar;
pub use user_menu::UserMenu;
