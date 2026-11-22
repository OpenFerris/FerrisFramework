//! Dashboard layouts: the Desk half of the layout endpoints (see
//! `ferris-api::dashboard_layout` and `ferris-api::dashboard_layout_doc`).
//!
//! The Desk renders a dashboard from the *effective* layout the server answers
//! with — the saved override when one exists, else the app's declared seed — so
//! these are the only data fns the editor needs: load, save the full widget
//! list **and the board configuration**, and delete the saved row to fall back
//! to the seed.
//!
//! Two backings, one call surface: [`LayoutTarget`] says where the effective
//! layout lives, and the three fns below take it, so `DashboardGrid` does not
//! branch its requests per dashboard kind.
//!
//! The Desk's **global** board style (one shared row, the base of every board's
//! precedence) lives at `/api/dashboard-settings`; [`dashboard_settings`] and
//! [`save_dashboard_settings`] are its two calls, kept here because they are the
//! same subject: how a board looks.

use ferris_schema::contract::WidgetInstance;
use ferris_schema::contract::{
    DashboardConfig, DashboardLayout, DashboardLayoutPayload, DashboardSettings,
    DashboardSettingsPayload, DashboardStyle,
};
use ferris_web_shared::route::path_segment;

use crate::api::client::{api_json, api_request_raw};

/// Where one dashboard's effective layout is read from and written to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayoutTarget {
    /// An app dashboard: the shared override of `(owner, id)` when one
    /// exists, else the seed the app declares (or the `Dashboard` document
    /// named `id` for the native `db` owner).
    App { owner: String, id: String },
    /// A document that *is* its own dashboard (the `document` binding): the
    /// doctype's bound `widgets` and `config` fields **are** the layout, read
    /// and written in place at
    /// `GET|PUT /api/dashboard-layout/doc/{doctype}/{name}` — no per-user
    /// override, so what one user saves is what every user sees.
    Document { doctype: String, name: String },
}

impl LayoutTarget {
    /// The endpoint this target is read from and written to.
    ///
    /// `owner`, `id`, `doctype` and `name` are data (an app id or a docname can
    /// contain `/`, `?` or `#`), so each goes through [`path_segment`]; a raw
    /// interpolation would let a document dashboard named `a/b` address a
    /// different route than the one the caller picked.
    fn path(&self) -> String {
        match self {
            Self::App { owner, id } => format!(
                "/api/dashboard-layout/{}/{}",
                path_segment(owner),
                path_segment(id)
            ),
            Self::Document { doctype, name } => format!(
                "/api/dashboard-layout/doc/{}/{}",
                path_segment(doctype),
                path_segment(name)
            ),
        }
    }
}

/// Fetch the effective layout for one dashboard.
///
/// Requires a session — the layout is only answerable for a caller who may read
/// the Desk — so it fails for a guest rather than answering with a grid nobody
/// asked for.
pub async fn dashboard_layout(target: &LayoutTarget) -> Result<DashboardLayout, String> {
    let value = api_json("GET", &target.path(), None).await?;
    serde_json::from_value(value).map_err(|e| e.to_string())
}

/// Save the arrangement **and the board configuration** of one dashboard.
///
/// The answer is the state the server actually saved (unresolvable references
/// dropped, spans clamped, config numbers clamped), not a bare acknowledgement,
/// so the desk can draw exactly what the next load will reproduce.
pub async fn save_layout(
    target: &LayoutTarget,
    widgets: &[WidgetInstance],
    config: &DashboardConfig,
) -> Result<DashboardLayout, String> {
    let payload = DashboardLayoutPayload {
        widgets: widgets.to_vec(),
        config: config.clone(),
    };
    let body = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
    let (status, text) = api_request_raw("PUT", &target.path(), Some(&body)).await?;
    if !(200..300).contains(&status) {
        return Err(format!("{status}: {text}"));
    }
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// Fetch the Desk's global board style and whether the caller may edit it.
///
/// One row shared by the install, so every board layers its own declared style
/// and saved override over this base. A failure is not fatal: the board renders
/// with the built-in defaults, exactly as it did before the global existed.
pub async fn dashboard_settings() -> Result<DashboardSettings, String> {
    let value = api_json("GET", "/api/dashboard-settings", None).await?;
    serde_json::from_value(value).map_err(|e| e.to_string())
}

/// Store the global board style and get back what the server saved (numbers
/// clamped), so the Desk repaints with exactly what the next load reproduces.
pub async fn save_dashboard_settings(style: &DashboardStyle) -> Result<DashboardSettings, String> {
    let payload = DashboardSettingsPayload {
        style: style.clone(),
    };
    let body = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
    let (status, text) = api_request_raw("PUT", "/api/dashboard-settings", Some(&body)).await?;
    if !(200..300).contains(&status) {
        return Err(format!("{status}: {text}"));
    }
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// Drop the saved row and get back the app's declared seed.
///
/// Only meaningful for [`LayoutTarget::App`]: a document dashboard has no row
/// to drop (its fields are the state), and its endpoint answers 405 rather than
/// pretending to restore anything.
pub async fn restore_layout(target: &LayoutTarget) -> Result<DashboardLayout, String> {
    let (status, text) = api_request_raw("DELETE", &target.path(), None).await?;
    if !(200..300).contains(&status) {
        return Err(format!("{status}: {text}"));
    }
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::LayoutTarget;

    /// A dashboard id or a docname is data: `/` or `?` inside it must not add a
    /// segment or rewrite the query of the layout endpoint.
    #[test]
    fn layout_paths_encode_every_dynamic_segment() {
        let app = LayoutTarget::App {
            owner: "sales app".into(),
            id: "a/b?c".into(),
        };
        assert_eq!(app.path(), "/api/dashboard-layout/sales%20app/a%2Fb%3Fc");

        let document = LayoutTarget::Document {
            doctype: "Sales Invoice".into(),
            name: "a/b?c#d".into(),
        };
        assert_eq!(
            document.path(),
            "/api/dashboard-layout/doc/Sales%20Invoice/a%2Fb%3Fc%23d"
        );
        assert!(!document.path().contains('?'));
    }
}
