//! `RealHostApi::http_request`: enforce the policy, then perform the call.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};

use super::policy::{GuardedResolver, clamp_timeout_ms, host_in};
use crate::host::{OutboundHttpRequest, OutboundHttpResponse};
use crate::host_impl::RealHostApi;
use crate::tls::build_client_config;

impl RealHostApi {
    // -- outbound http ------------------------------------------------------

    pub(crate) async fn http_request(
        &self,
        request: &OutboundHttpRequest,
    ) -> Result<OutboundHttpResponse> {
        // Enforce the allow-list before touching the network. Every rejection
        // is a plain anyhow error, which the plugin boundary classifies as
        // validation-failed (422) — the plugin asked for something the host
        // policy refuses, not something that broke.
        let url = reqwest::Url::parse(&request.url)
            .map_err(|e| anyhow::anyhow!("cannot parse url `{}`: {e}", request.url))?;
        if !matches!(url.scheme(), "http" | "https") {
            anyhow::bail!("http request must use http(s), got `{}`", url.scheme());
        }
        let host = url
            .host_str()
            .ok_or_else(|| anyhow::anyhow!("http url `{}` has no host", request.url))?
            .to_string();
        let allow_private = request.unrestricted && request.allow_private;
        self.http.policy_check(
            &host,
            &request.method,
            &request.headers,
            request.body.as_deref(),
            request.unrestricted,
            request.allow_credentials,
        )?;
        if allow_private {
            self.http.resolve_and_check_private(&url, true).await?;
        } else if !request.unrestricted {
            // A name that resolves to loopback/RFC 1918/link-local must not
            // sneak past an allow-list that only names hosts.
            self.http.resolve_and_check_private(&url, false).await?;
        } // The call may narrow where it goes; it can never widen it. An empty
        // list is "no narrowing", which is why it is skipped instead of
        // matching nothing.
        if !request.allowed_hosts.is_empty() && !host_in(&request.allowed_hosts, &host) {
            anyhow::bail!("host `{host}` is outside this call's `http-policy.allowed-hosts`");
        }

        let method = reqwest::Method::from_bytes(request.method.as_bytes())
            .map_err(|e| anyhow::anyhow!("invalid http method `{}`: {e}", request.method))?;
        let client = self.http.client_for(
            request.redirect_policy.as_deref(),
            request.redirect_hops,
            allow_private,
        )?;

        // TLS: only `https` carries material, and the bridge already checked
        // the `tls-client` grant. A per-call `tls` replaces the shared client,
        // because the connector is built from the call's own material.
        let (client, url) = match request.tls.as_ref() {
            Some(tls) if url.scheme() == "https" => {
                let config = build_client_config(tls)?;
                let redirects = self
                    .http
                    .redirect_policy(request.redirect_policy.as_deref(), request.redirect_hops)?;
                let builder = reqwest::Client::builder()
                    .redirect(redirects)
                    // The per-call TLS client needs the same connect-time guard:
                    // the redirects it follows are resolved by this client.
                    .dns_resolver(Arc::new(GuardedResolver { allow_private }))
                    .use_preconfigured_tls((*config).clone());
                // SNI override: reqwest takes the name from the URL, so the URL
                // is rewritten to the declared name and pinned back to the
                // address the *original* host resolved to. `Host` keeps the
                // original name, so the server still sees what was asked for.
                let (builder, url) = match tls.server_name.as_deref() {
                    Some(name) if !name.trim().is_empty() && name != host => {
                        let port = url
                            .port_or_known_default()
                            .ok_or_else(|| anyhow::anyhow!("`{}` has no port", request.url))?;
                        let addr = self.http.resolve_first(&host, port).await?;
                        let mut rewritten = url.clone();
                        rewritten.set_host(Some(name)).map_err(|e| {
                            anyhow::anyhow!("cannot apply server-name `{name}`: {e}")
                        })?;
                        (builder.resolve(name, addr), rewritten)
                    }
                    _ => (builder, url),
                };
                let client = builder
                    .build()
                    .context("building the per-call TLS client")?;
                (client, url)
            }
            Some(_) => {
                // Material on a plain-http call cannot be honoured, and
                // silently dropping it would be a false success.
                anyhow::bail!(
                    "tls material was declared for a `{}` url: only https carries it",
                    url.scheme()
                );
            }
            None => (client.clone(), url),
        };

        let mut builder = client.request(method, url);
        for (name, value) in &request.headers {
            builder = builder.header(name, value);
        }
        if let Some(body) = &request.body {
            builder = builder.body(body.clone());
        }
        let timeout_ms = clamp_timeout_ms(request.timeout_ms, self.http.default_timeout_ms);
        // `.context` (not `format!`) keeps reqwest's own chain, so a TLS
        // verification failure is visible in the log instead of collapsing into
        // "error sending request".
        let mut response = builder
            .timeout(Duration::from_millis(timeout_ms))
            .send()
            .await
            .context("http request failed")?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .filter_map(|(name, value)| {
                value
                    .to_str()
                    .ok()
                    .map(|text| (name.as_str().to_string(), text.to_string()))
            })
            .collect::<Vec<_>>();
        // Reject an oversized body before reading a byte when the server is
        // honest enough to declare it.
        let cap = self.http.effective_max_body(request.max_body);
        if let Some(declared) = response.content_length()
            && declared > u64::try_from(cap).unwrap_or(u64::MAX)
        {
            anyhow::bail!("http response declares {declared} bytes, over the {cap} byte limit");
        }
        // Enforce the cap *while* streaming: an oversized (or hostile) response
        // is aborted as soon as it crosses the limit instead of being buffered
        // whole and only then rejected (R-FW-2).
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| anyhow::anyhow!("cannot read http response body: {e}"))?
        {
            if body.len().saturating_add(chunk.len()) > cap {
                anyhow::bail!("http response body exceeds the {cap} byte limit");
            }
            body.extend_from_slice(&chunk);
        }
        Ok(OutboundHttpResponse {
            status,
            headers,
            body,
        })
    }
}
