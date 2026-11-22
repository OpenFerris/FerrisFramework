//! Operation-scoped host context: the ambient user, the certificate marker,
//! the engine context derived from them, and the in-app notifications written
//! as the service principal.
//!
//! A verified client certificate is **not** a global service authorization.
//! The request boundary (`plugin::exports`) resolves it to a fail-closed
//! service principal — `cert:<fingerprint>`, no roles — and binds that as the
//! ambient user, so the engine still evaluates `DocPerm`/`Everyone` against an
//! ordinary identity. A certificate that carries neither a fingerprint nor a
//! CN produces no identity at all and the engine default-denies. Granting a
//! device access is an operator decision: map its fingerprint (or CN) to a
//! `User` or a role, exactly like any other principal.

use super::RealHostApi;
use crate::host::UserContext;
use anyhow::Result;
use ferris_auth::user::User;
use ferris_core::Value;
use ferris_document::engine::EngineContext;

impl RealHostApi {
    /// The host user behind the current operation (plugin ambient identity).
    pub(super) fn current_user(&self) -> Option<User> {
        self.current_user.lock().clone()
    }

    /// Whether the current operation arrived over a verified client certificate
    /// and carried no session.
    ///
    /// This is a **diagnostic marker**, never an authorization input: the
    /// service identity is resolved and bound at the boundary, and the engine
    /// only ever sees [`EngineContext::user`]. It lets [`Self::ctx`] warn when a
    /// certificate carried no usable identity instead of failing silently.
    pub(super) fn certificate_service(&self) -> bool {
        *self.certificate_service.lock()
    }

    /// Resolve verified client-certificate facts to the fail-closed service
    /// principal its operations run as.
    ///
    /// A certificate authenticates a **device**, not a user, and a device is
    /// not an administrator. The principal is `User`-shaped so the engine can
    /// evaluate its ordinary policy (`DocPerm`, `Everyone`, owner rules), but
    /// it carries **no roles**: unless the operator maps the certificate to a
    /// user or grants a role, every unpermitted doctype read/write is denied.
    ///
    /// The id is stable for the certificate's lifetime — `cert:<fingerprint>`,
    /// or `cert:<CN>` when the transport reported no fingerprint — so reconnects
    /// keep the same identity and a renewed certificate gets a new one. `None`
    /// means the certificate carried neither fact: no identity is fabricated
    /// and the engine default-denies.
    #[must_use]
    pub(crate) fn certificate_service_user(
        fingerprint: &str,
        common_name: Option<&str>,
    ) -> Option<User> {
        let fingerprint = fingerprint.trim();
        let common_name = common_name.map_or("", str::trim);
        let material = if fingerprint.is_empty() {
            common_name
        } else {
            fingerprint
        };
        if material.is_empty() {
            return None;
        }
        let id = format!("cert:{material}");
        let mut user = User::system_user(
            format!("{material}@certificate.local"),
            format!("Client certificate {material}"),
        );
        // `User::system_user` mints a random UUID; the certificate facts are
        // the identity, so replace it with the stable derived id.
        user.id = id;
        // Explicit and load-bearing: a certificate grants no role. The check
        // falls through to the doctype policy instead of a blanket allow.
        user.roles.clear();
        Some(user)
    }

    /// Create `EngineContext` with the current user context.
    ///
    /// Authorization is decided solely by `ctx.user`: the boundary binds the
    /// device's service principal (see
    /// [`Self::certificate_service_user`]) and this function never opts into
    /// `allow_anonymous`. A certificate operation that reaches here without a
    /// user identifies nothing, so the engine's default-deny is the correct
    /// outcome; the marker only makes that visible in the log.
    pub(super) fn ctx(&self, doctype: &str) -> EngineContext<'_> {
        let mut ctx = EngineContext::new(doctype, "");
        ctx.user = self.current_user();
        if ctx.user.is_none() && self.certificate_service() {
            tracing::warn!(
                doctype,
                "certificate operation without a device identity (no fingerprint or CN); \
                 the engine will deny"
            );
        }
        ctx
    }

    /// The user as plugins see it. Anonymous means an honest guest — never a
    /// fabricated administrator.
    pub(super) fn to_user_context(user: Option<User>) -> UserContext {
        if let Some(user) = user {
            UserContext {
                name: user.id,
                email: user.email,
                roles: user.roles,
            }
        } else {
            let guest = User::guest();
            UserContext {
                name: guest.id,
                email: guest.email,
                roles: guest.roles,
            }
        }
    }

    /// The service principal plugin-side in-app notifications are written as —
    /// the `System Manager` twin of the API layer's `panel::service_user`.
    /// A plugin send is a platform action, not the (anonymous, permission-less)
    /// guest's: the engine's `Create` check on `Notification` refuses
    /// `User::guest()`, and pre-authz that silently dropped every in-app
    /// notification the contract reported as delivered (§C2).
    pub(super) fn notification_service_user() -> User {
        let mut user = User::system_user("system@ferris.local".to_string(), "System".to_string());
        user.roles = vec!["System Manager".to_string()];
        user
    }

    /// In-app notification: a `Notification` document owned by the recipient,
    /// exactly as the Desk feed reads it. Best-effort: a bare plugin host
    /// without the engine still logs and moves on.
    pub(super) fn notify_in_app(&self, to: &str, title: &str, message: &str) {
        let result = crate::setup::block_on(self.notify_in_app_async(to, title, message));
        if let Err(e) = result {
            tracing::warn!(to, error = %e, "in-app notification not persisted");
        }
    }

    pub(super) async fn notify_in_app_async(
        &self,
        to: &str,
        title: &str,
        message: &str,
    ) -> Result<()> {
        let data = serde_json::json!({
            "user": to,
            "notification_type": "General",
            "title": title,
            "description": message,
            "route": "/",
            "is_read": false,
        });
        let mut ctx = EngineContext::new("Notification", "");
        ctx.user = Some(Self::notification_service_user());
        let created = self
            .engine
            .create_owned_by(&ctx, to, Value::from_json(data))
            .await?;
        // Fan the stored row out to any live Desktop session over the
        // process-wide hook, exactly as `ferris-api`'s producer does. The wire
        // shape mirrors the Desk feed (`read`, not `is_read`).
        let payload = serde_json::json!({
            "id": created.get("name").and_then(|v| v.as_str()).unwrap_or(""),
            "title": title,
            "description": message,
            "route": "/",
            "created_at": created.get("creation").and_then(|v| v.as_str()).unwrap_or(""),
            "read": false,
        });
        ferris_core::notifications::publish(to, Value::from_json(payload));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::RealHostApi;
    use ferris_core::permissions::{Permission, Resource, check_permission};
    use ferris_document::engine::EngineContext;

    /// A verified certificate binds a service principal, never the global
    /// `allow_anonymous` bypass: with no role it cannot read or write a doctype
    /// that has no `DocPerm`, which is exactly what the engine sees.
    #[test]
    fn certificate_without_session_binds_a_fail_closed_service_principal() {
        let user = RealHostApi::certificate_service_user("deadbeef", Some("SOLMEC-001"))
            .expect("a fingerprint yields a service identity");
        assert_eq!(
            user.id, "cert:deadbeef",
            "the id is derived from the certificate"
        );
        assert!(user.roles.is_empty(), "a certificate must not carry roles");

        let mut ctx = EngineContext::new("Equipo", "");
        ctx.user = Some(user.clone());
        assert!(
            !ctx.allow_anonymous,
            "certificate mode must not enable the global anonymous bypass"
        );
        assert!(
            ctx.user.is_some(),
            "the context carries the device principal, not `None`"
        );

        let resource = Resource::new("Equipo", "H7-1");
        assert!(
            !check_permission(&user, &resource, Permission::Read),
            "a role-less device cannot read a doctype without a DocPerm"
        );
        assert!(
            !check_permission(&user, &resource, Permission::Write),
            "a role-less device cannot write a doctype without a DocPerm"
        );
    }

    /// The CN is the only fallback when the transport reported no fingerprint;
    /// the id stays stable across reconnects for the same certificate.
    #[test]
    fn certificate_without_fingerprint_falls_back_to_the_common_name() {
        let user = RealHostApi::certificate_service_user("", Some("SOLMEC-001"))
            .expect("a CN yields a service identity");
        assert_eq!(user.id, "cert:SOLMEC-001");
        assert!(user.roles.is_empty());
    }

    /// Nothing to derive means nothing to authorize: no identity is fabricated
    /// and the engine default-denies the request.
    #[test]
    fn certificate_without_fingerprint_or_cn_binds_no_identity() {
        assert!(RealHostApi::certificate_service_user("", None).is_none());
        assert!(
            RealHostApi::certificate_service_user("   ", Some("   ")).is_none(),
            "blank facts are not an identity"
        );
    }
}
