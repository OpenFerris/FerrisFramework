//! `notification` interface of [`HostApi`](crate::host::HostApi).

use super::RealHostApi;
use crate::host::notification_sender;
use anyhow::Result;

impl RealHostApi {
    // -- notification -------------------------------------------------------

    pub(super) async fn notification_send(
        &self,
        to: &str,
        channel: &str,
        title: &str,
        message: &str,
    ) -> Result<()> {
        let channels: Vec<&str> = channel
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|c| !c.is_empty())
            .collect();
        if channels.is_empty() {
            anyhow::bail!("notification channel must be one of: email, push, in-app, webhook, all");
        }
        for requested in &channels {
            match *requested {
                "all" => {
                    self.notify_in_app(to, title, message);
                    if let Some(sender) = notification_sender() {
                        sender.send(to, "email", title, message)?;
                        sender.send(to, "webhook", title, message)?;
                    } else {
                        tracing::warn!(
                            to,
                            "email/webhook not delivered: no notification sender configured"
                        );
                        // Tell the guest instead of claiming success: dropping
                        // email/webhook silently is how this stayed broken
                        //.
                        anyhow::bail!(
                            "in-app notification delivered, but email/webhook were not: \
                             the host has no notification sender configured"
                        );
                    }
                }
                "push" | "in-app" => self.notify_in_app(to, title, message),
                "email" | "webhook" => match notification_sender() {
                    Some(sender) => sender.send(to, requested, title, message)?,
                    None => {
                        tracing::warn!(
                            channel = *requested,
                            to,
                            "not delivered: no notification sender configured"
                        );
                        anyhow::bail!(
                            "notification channel `{requested}` has no delivery backend: \
                             the host has no notification sender configured"
                        );
                    }
                },
                other => {
                    anyhow::bail!(
                        "unsupported notification channel `{other}` (use email, push, in-app, webhook or all)"
                    )
                }
            }
        }
        Ok(())
    }
}
