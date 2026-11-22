//! `naming` and `session` interfaces of [`HostApi`](crate::host::HostApi), plus
//! the ambient identity the operation runs as.

use super::RealHostApi;
use crate::host::UserContext;
use anyhow::Result;
use ferris_auth::permissions::{Resource, check_permission};
use ferris_auth::user::User;
use ferris_document::engine::EngineContext;
use ferris_document::naming::NamingSeriesStore;

impl RealHostApi {
    // -- identity -----------------------------------------------------------

    pub(super) async fn naming_next(&self, series: &str) -> Result<String> {
        Ok(NamingSeriesStore::next(&self.pool, series).await?)
    }

    pub(super) async fn session_current(&self) -> Result<UserContext> {
        Ok(Self::to_user_context(self.current_user()))
    }

    pub(super) async fn session_can(
        &self,
        doctype: &str,
        action: &str,
        name: Option<&str>,
    ) -> Result<bool> {
        let user = self.current_user().unwrap_or_else(User::guest);

        // Resolve the row's owner once. A missing row and a denied row answer
        // the same: the guest learns the verdict, never whether an arbitrary id
        // exists (existence oracle).
        let owner = match name {
            Some(row_name) => {
                // Row-level check: resolve the row's owner under the system
                // service principal (the same identity in-app notifications are
                // written as), then evaluate the policy with `if_owner` bound
                // so row ownership grants fire. The caller only ever learns
                // the yes/no — never the owner.
                let mut system = EngineContext::new(doctype, "");
                system.user = Some(Self::notification_service_user());
                let value = match self.engine.get(&system, row_name).await {
                    Ok(value) => value,
                    Err(ferris_core::Error::Document(ferris_core::DocumentError::NotFound {
                        ..
                    })) => return Ok(false),
                    Err(e) => return Err(e.into()),
                };
                Some(
                    value
                        .get("owner")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                )
            }
            None => None,
        };

        // `Permission` implements `std::str::FromStr`; a name outside the
        // built-in rights is a **custom permission type**
        // (`Permission Type`): resolve it against the same effective
        // `Custom DocPerm` rows the Desk renders, so `approve` works through
        // `platform::can` exactly like `read`.
        let Ok(action_enum) = action.parse::<ferris_auth::permissions::Permission>() else {
            return Ok(ferris_schema::has_custom_permission_on(
                &self.pool,
                &user,
                doctype,
                owner.as_deref(),
                action,
            )
            .await?);
        };

        let resource = match (name, owner) {
            (Some(row_name), Some(owner)) => Resource::new(doctype, row_name).with_owner(owner),
            _ => Resource::new(doctype, "*"),
        };
        Ok(check_permission(&user, &resource, action_enum))
    }

    pub(super) async fn session_can_many(
        &self,
        checks: &[(String, String, Option<String>)],
    ) -> Result<Vec<bool>> {
        // Reuses the single-check path (including its row-owner resolution);
        // the win is one boundary crossing for a whole list render, not a
        // different policy.
        let mut out = Vec::with_capacity(checks.len());
        for (doctype, action, name) in checks {
            out.push(self.session_can(doctype, action, name.as_deref()).await?);
        }
        Ok(out)
    }

    // -- identity attachment ------------------------------------------------

    pub(super) fn set_current_user(&self, user: Option<User>) {
        *self.current_user.lock() = user;
    }

    pub(super) fn set_certificate_service(&self, active: bool) {
        *self.certificate_service.lock() = active;
    }
}
