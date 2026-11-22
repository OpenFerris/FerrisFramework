//! `platform` import bridge: who is calling,
//! what the app may configure, the host services it may schedule or sign, and
//! which verticals this host binary was actually compiled with.
//!
//! The two identity questions — `current-user` and `can` — are deliberately
//! **ungated**: the WIT contract says reading the caller's own identity needs
//! no capability, and a permission *question* always gets an honest answer
//! (the engine decides what the user sees, the capability decides whether the
//! app may call a service at all). Everything else goes through
//! [`HostState::require`](crate::host::HostState::require).

use crate::ferris::framework::platform as wit_platform;
use crate::ferris::framework::types::Error as WitError;
use crate::ferris::framework::types::GrantSet;
use crate::host::conversions::cert_usage_from_wit;
use crate::host::{Capability, CertUsage, HostState, ProcessSpec, ScheduleInfo, UserContext};

use super::block_on;

impl crate::ferris::framework::platform::Host for HostState {
    fn current_user(&mut self) -> Result<wit_platform::User, WitError> {
        block_on(async {
            let user = self.api().session_current().await?;
            Ok(UserContext::into_wit(user))
        })
    }

    fn can(&mut self, checks: Vec<wit_platform::PermissionCheck>) -> Result<Vec<bool>, WitError> {
        let checks: Vec<(String, String, Option<String>)> = checks
            .into_iter()
            .map(|check| (check.doctype, check.action, check.name))
            .collect();
        block_on(async { self.api().session_can_many(&checks).await })
    }

    fn grants(&mut self) -> Result<GrantSet, WitError> {
        // The effective grant, after intersecting the declaration with the
        // effective set: the app branches on this instead of probing for
        // failures.
        Ok(self.grant_set().to_wit())
    }

    fn features(&mut self) -> Result<Vec<String>, WitError> {
        // Feature detection, not a grant: what the **running** host compiled.
        // Ungated on purpose — an app that cannot ask which surfaces exist
        // cannot decide whether to call them, and the answer reveals no tenant
        // data. The set is installed once at boot in
        // `ferris_core::host_features`; a process that never installed one
        // reports an empty list, so a guest hides the optional surfaces instead
        // of calling a service that would answer `unsupported`.
        let mut names: std::collections::BTreeSet<String> = crate::host::host_features()
            .iter()
            .map(str::to_string)
            .collect();
        // `threads` is the one engine surface this crate owns: the server's
        // compile-time set cannot know whether this process's engine enabled
        // the proposal, so the runtime adds it here (and only when it did).
        if self.threads_supported() {
            names.insert("threads".to_string());
        }
        Ok(names.into_iter().collect())
    }

    fn config_get(&mut self, key: String) -> Result<Option<String>, WitError> {
        self.require(Capability::Config)?;
        // `secret` config keys are operator-owned: the guest may not read one,
        // whatever it declares. The app keeps its own secrets in its world
        // (`/world/secrets`); this store is for material the host resolves
        // itself, such as TLS PEM behind `tls-material.secret`.
        if self.is_secret_config_key(&key) {
            return Err(WitError::PermissionDenied(format!(
                "config key `{key}` is `secret`: store the app's own secrets in /world/secrets"
            )));
        }
        let plugin = self.plugin_name.clone();
        block_on(async { self.api().config_get(&plugin, &key).await })
    }

    fn config_set(&mut self, key: String, value: String) -> Result<(), WitError> {
        self.require(Capability::Config)?;
        if self.is_secret_config_key(&key) {
            return Err(WitError::PermissionDenied(format!(
                "config key `{key}` is `secret`: the guest cannot write operator-owned material"
            )));
        }
        let plugin = self.plugin_name.clone();
        block_on(async { self.api().config_set(&plugin, &key, &value).await })
    }

    fn config_list(&mut self) -> Result<Vec<String>, WitError> {
        self.require(Capability::Config)?;
        let plugin = self.plugin_name.clone();
        block_on(async { self.api().config_list(&plugin).await })
    }

    fn config_delete(&mut self, key: String) -> Result<(), WitError> {
        self.require(Capability::Config)?;
        if self.is_secret_config_key(&key) {
            return Err(WitError::PermissionDenied(format!(
                "config key `{key}` is `secret`: the guest cannot delete operator-owned material"
            )));
        }
        let plugin = self.plugin_name.clone();
        block_on(async { self.api().config_delete(&plugin, &key).await })
    }

    fn schedule(&mut self, payload: String) -> Result<String, WitError> {
        self.require(Capability::Scheduler)?;
        let plugin = self.plugin_name.clone();
        block_on(async { self.api().scheduler_schedule(&plugin, &payload).await })
    }

    fn enqueue_job(&mut self, job_type: String, payload: String) -> Result<(), WitError> {
        self.require(Capability::Scheduler)?;
        let plugin = self.plugin_name.clone();
        block_on(async {
            self.api()
                .scheduler_enqueue(&plugin, &job_type, &payload)
                .await
        })
    }

    fn cancel_schedule(&mut self, schedule_id: String) -> Result<(), WitError> {
        self.require(Capability::Scheduler)?;
        let plugin = self.plugin_name.clone();
        block_on(async { self.api().scheduler_cancel(&plugin, &schedule_id).await })
    }

    fn list_schedules(&mut self) -> Result<Vec<wit_platform::ScheduleInfo>, WitError> {
        self.require(Capability::Scheduler)?;
        let plugin = self.plugin_name.clone();
        block_on(async {
            self.api()
                .scheduler_list(&plugin)
                .await
                .map(|schedules| schedules.into_iter().map(ScheduleInfo::into_wit).collect())
        })
    }

    fn sign_csr(
        &mut self,
        csr_der_b64: String,
        ttl_seconds: u32,
        usage: wit_platform::CertUsage,
    ) -> Result<String, WitError> {
        self.require(Capability::Pki)?;
        let usage = cert_usage_from_wit(usage);
        if usage == CertUsage::Server {
            // A server certificate is only useful if the app may actually serve
            // TLS with it, so the two grants travel together.
            self.require(Capability::TlsServer)?;
        }
        let plugin = self.plugin_name.clone();
        block_on(async {
            self.api()
                .pki_sign_csr_usage(&plugin, &csr_der_b64, ttl_seconds, usage)
                .await
        })
    }

    fn ca_cert(&mut self) -> Result<String, WitError> {
        self.require(Capability::Pki)?;
        block_on(async { self.api().pki_ca_cert().await })
    }

    fn revoke_cert(&mut self, fingerprint: String) -> Result<(), WitError> {
        self.require(Capability::Pki)?;
        let plugin = self.plugin_name.clone();
        block_on(async { self.api().pki_revoke(&plugin, &fingerprint).await })
    }

    fn is_revoked(&mut self, fingerprint: String) -> Result<bool, WitError> {
        self.require(Capability::Pki)?;
        block_on(async { self.api().pki_is_revoked(&fingerprint).await })
    }

    // -- host programs ------------------------------------------------------
    //
    // `process` is specific: running a program executes host code on the app's
    // behalf, so declaring it is the request and the grant. The child belongs
    // to the host (the running server's supervisor), never to the guest: the
    // bridge only forwards, and a runtime without a supervisor answers the
    // typed `unsupported` failure.

    fn process_run(
        &mut self,
        spec: wit_platform::ProcessSpec,
    ) -> Result<wit_platform::ProcessOutput, WitError> {
        self.require(Capability::Process)?;
        let spec = ProcessSpec::from(spec);
        let plugin = self.plugin_name.clone();
        block_on(async { self.api().process_run(&plugin, &spec).await.map(Into::into) })
    }

    fn process_spawn(&mut self, spec: wit_platform::ProcessSpec) -> Result<String, WitError> {
        self.require(Capability::Process)?;
        let spec = ProcessSpec::from(spec);
        let plugin = self.plugin_name.clone();
        block_on(async { self.api().process_spawn(&plugin, &spec).await })
    }

    fn process_wait(
        &mut self,
        process_id: String,
    ) -> Result<wit_platform::ProcessOutput, WitError> {
        self.require(Capability::Process)?;
        let plugin = self.plugin_name.clone();
        block_on(async {
            self.api()
                .process_wait(&plugin, &process_id)
                .await
                .map(Into::into)
        })
    }

    fn process_kill(&mut self, process_id: String) -> Result<(), WitError> {
        self.require(Capability::Process)?;
        let plugin = self.plugin_name.clone();
        block_on(async { self.api().process_kill(&plugin, &process_id).await })
    }
}

impl UserContext {
    fn into_wit(self) -> wit_platform::User {
        self.into()
    }
}

impl ScheduleInfo {
    fn into_wit(self) -> wit_platform::ScheduleInfo {
        wit_platform::ScheduleInfo {
            id: self.id,
            job_name: self.job_name,
            cron: self.cron,
            enabled: self.enabled,
        }
    }
}
