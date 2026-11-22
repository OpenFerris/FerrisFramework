//! `http` interface of [`HostApi`](crate::host::HostApi) and the outbound
//! policy it enforces (allow-list, size caps, timeout, header hygiene, TLS).
//!
//! The effective policy of one call is the strictest of three inputs: what the
//! guest declared, what the operator allows and what the host caps. Two of them
//! are decided before this file runs — the bridge refuses a call without the
//! `http` capability, and it marks the call `unrestricted` only when the app
//! holds `http-unrestricted`. What is left here is enforcement: an unrestricted
//! call skips `allowed-hosts`/`allow-private` and keeps every size, timeout and
//! redirect cap, exactly as the WIT documents.
//!
//! [`request`] is the call path; [`policy`] owns the allow-list, the
//! connect-time resolver and the per-call client.

mod policy;
mod request;

#[cfg(test)]
mod tests;

pub use policy::HttpPolicy;
#[cfg(test)]
pub use policy::{DEFAULT_HTTP_MAX_BODY_BYTES, MAX_HTTP_TIMEOUT_MS, clamp_timeout_ms};
