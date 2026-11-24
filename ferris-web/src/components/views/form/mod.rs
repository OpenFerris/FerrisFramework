//! Form view components.

pub mod child_table;
pub mod customize;
pub mod edit_mode;
pub mod engagement;
pub mod field_renderer;
pub mod field_types;
pub mod layout;
pub mod sidebar;
pub mod sidebar_extra;
pub mod topbar;
pub mod tour;

pub use child_table::ChildTableField;
pub use edit_mode::{FieldEditCtx, InspectorPanel};
pub use engagement::FormEngagement;
pub use field_renderer::render_form_field;
pub use layout::{FormSectionView, FormTabs};
pub use sidebar::FormSidebar;
pub use sidebar_extra::{ConnectionsPanel, DocTypeActions};
pub use topbar::FormTopBar;
pub use tour::FormTourLauncher;
