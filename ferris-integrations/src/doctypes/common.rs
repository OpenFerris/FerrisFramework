//! Shared module constant and the permission helpers.

use ferris_schema::RolePermission;

/// Desk module/section every DocType here belongs to.
pub const MODULE: &str = "Integrations";

/// Full CRUD for the administrator role.
#[must_use]
pub(super) fn system_manager() -> RolePermission {
    RolePermission::new("System Manager")
        .read()
        .write()
        .create()
        .delete()
}

/// Read + write with neither create nor delete: the policy of a Single.
///
/// A Single's one row is created lazily by `get_single`, which deliberately
/// skips `Permission::Create` (§13), so a `create` grant here would be a right
/// no code path ever checks.
#[must_use]
pub(super) fn system_manager_single() -> RolePermission {
    RolePermission::new("System Manager").read().write()
}
