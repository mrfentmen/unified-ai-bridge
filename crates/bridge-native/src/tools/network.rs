//! Explicitly reviewed, anonymous public HTTP(S) GETs. No browser session access.
mod text;
use super::{binary, parse_args, ToolContext, ToolOutcome};
use crate::{
    app::App,
    failure::ToolFailure,
    fsops::{sha256_bytes, PathLock},
};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    io::Write,
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};
use url::{Host, Url};

const TEXT_BYTES: usize = 512 * 1024;
const FILE_BYTES: usize = 8 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(20);
static SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);
fn invalid(s: &str) -> ToolFailure {
    ToolFailure::InvalidArguments(s.into())
}
fn denied(s: &str) -> ToolFailure {
    ToolFailure::PermissionDenied(s.into())
}
fn internal(e: impl std::fmt::Display) -> ToolFailure {
    ToolFailure::Internal(e.to_string())
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Policy {
    pub enabled: bool,
    pub origins: Vec<String>,
    pub revision: String,
}
impl Policy {
    pub fn validate(&self) -> Result<()> {
        if self.origins.len() > 20 || (self.enabled && self.origins.is_empty()) {
            bail!("network grants require 1–20 origins when enabled");
        }
        if (!self.revision.is_empty() || self.enabled)
            && (self.revision.len() != 32 || !self.revision.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            bail!("invalid network grant revision");
        }
        for origin in &self.origins {
            let url = validate_url(origin)?;
            if url.path() != "/" || url.origin().ascii_serialization() != *origin {
                bail!("network grants must contain normalized HTTP(S) origins only");
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum Control {
    Status,
    Save { enabled: bool, origins: Vec<String> },
}

pub fn policy(app: &App) -> Policy {
    app.config
        .workspace_network
        .get(&app.workspace.id())
        .cloned()
        .unwrap_or_default()
}

pub fn control(app: &Arc<App>, workspace: &str, action: Control) -> Result<(Arc<App>, Value)> {
    if workspace != app.workspace.id() {
        bail!("workspace changed; refresh network controls");
    }
    if let Control::Save { enabled, origins } = action {
        if origins.len() > 20 {
            bail!("at most 20 public origins can be granted");
        }
        let mut normalized = Vec::new();
        for origin in origins {
            let url = validate_url(&origin).map_err(anyhow::Error::from)?;
            if url.path() != "/" {
                bail!("grant origins only, without a URL path");
            }
            let origin = url.origin().ascii_serialization();
            if !normalized.contains(&origin) {
                normalized.push(origin);
            }
        }
        if enabled && normalized.is_empty() {
            bail!("choose at least one public origin before enabling network tools");
        }
        let policy = Policy {
            enabled,
            origins: normalized,
            revision: crate::approvals::ApprovalManager::new_id(),
        };
        policy.validate()?;
        app.audit_workspace(
            "network.policy",
            json!({"workspace_id":workspace,"policy":policy}),
        )?;
        let mut config = app.config.clone();
        config
            .workspace_network
            .insert(workspace.into(), policy.clone());
        app.home.save_config(&config)?;
        let next = Arc::new(App {
            home: app.home.clone(),
            config,
            workspace: app.workspace.clone(),
            store: app.store.clone(),
            approvals: app.approvals.clone(),
            events: app.events.clone(),
            interactive: app.interactive,
        });
        Ok((next, json!({"policy":policy})))
    } else {
        Ok((app.clone(), json!({"policy":policy(app)})))
    }
}

fn validate_url(raw: &str) -> Result<Url, ToolFailure> {
    if raw.len() > 2048 || raw.trim() != raw || raw.chars().any(char::is_control) {
        return Err(invalid(
            "URL must be at most 2048 bytes, without whitespace padding or control characters",
        ));
    }
    let url = Url::parse(raw).map_err(|_| invalid("invalid absolute HTTP(S) URL"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host().is_none() {
        return Err(denied("only public HTTP(S) URLs are supported"));
    }
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(denied("URL credentials, query strings and fragments are not supported; use an unauthenticated public URL"));
    }
    if url.port().is_some() {
        return Err(denied("only the standard HTTP/HTTPS port is supported"));
    }
    match url.host().unwrap() {
        Host::Ipv4(ip) if !public_ip(ip.into()) => {
            return Err(denied(
                "private, local and reserved network addresses are denied",
            ))
        }
        Host::Ipv6(ip) if !public_ip(ip.into()) => {
            return Err(denied(
                "private, local and reserved network addresses are denied",
            ))
        }
        Host::Domain(name)
            if !name.contains('.')
                || name.ends_with('.')
                || [
                    ".localhost",
                    ".local",
                    ".internal",
                    ".lan",
                    ".home",
                    ".test",
                    ".invalid",
                    ".example",
                    ".onion",
                ]
                .iter()
                .any(|s| name.ends_with(s)) =>
        {
            return Err(denied("local and special-use hostnames are denied"))
        }
        _ => {}
    }
    Ok(url)
}

fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => {
            let [a, b, c, d] = v.octets();
            !(a == 0
                || a == 10
                || a == 127
                || a >= 224
                || (a == 100 && (64..=127).contains(&b))
                || (a == 169 && b == 254)
                || (a == 172 && (16..=31).contains(&b))
                || (a == 192
                    && (b == 168 || (b == 0 && (c == 0 || c == 2)) || (b == 88 && c == 99)))
                || (a == 198 && (b == 18 || b == 19 || (b == 51 && c == 100)))
                || (a == 203 && b == 0 && c == 113)
                || [a, b, c, d] == [168, 63, 129, 16])
        }
        IpAddr::V6(v) => {
            let s = v.segments();
            (s[0] & 0xe000) == 0x2000
                && s[0] != 0x2002
                && !(s[0] == 0x2001 && (s[1] <= 0x01ff || s[1] == 0x0db8))
                && !(s[0] == 0x3fff && (s[1] & 0xf000) == 0)
        }
    }
}

fn check_policy(
    ctx: &ToolContext<'_>,
    expected: &Policy,
    url: &Url,
    writes: bool,
) -> Result<(), ToolFailure> {
    crate::tasks::checkpoint(ctx.app)?;
    let config = ctx
        .app
        .home
        .load_config()
        .map_err(|_| denied("network policy could not be reloaded"))?;
    let current = config
        .workspace_network
        .get(&ctx.app.workspace.id())
        .cloned()
        .unwrap_or_default();
    if !current.enabled || current != *expected {
        return Err(denied("network access is disabled or its grant changed"));
    }
    if !current
        .origins
        .contains(&url.origin().ascii_serialization())
    {
        return Err(denied(
            "URL origin is not in this workspace's public-network grant",
        ));
    }
    let workspace = ctx
        .app
        .store
        .workspaces()
        .map_err(internal)?
        .into_iter()
        .find(|w| w.id == ctx.app.workspace.id())
        .ok_or_else(|| denied("workspace grant was removed"))?;
    if writes && workspace.read_only {
        return Err(denied("workspace is locked read-only"));
    }
    Ok(())
}

async fn wait_revoked(ctx: &ToolContext<'_>, p: &Policy, url: &Url, writes: bool) -> ToolFailure {
    loop {
        if let Err(e) = check_policy(ctx, p, url, writes) {
            return e;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn validate_addresses(addresses: &[SocketAddr]) -> Result<(), ToolFailure> {
    if addresses.is_empty() || addresses.len() > 16 || addresses.iter().any(|a| !public_ip(a.ip()))
    {
        return Err(denied("DNS returned no usable public addresses, too many addresses, or a private/reserved address"));
    }
    Ok(())
}

async fn resolve(url: &Url) -> Result<Vec<SocketAddr>, ToolFailure> {
    let port = url.port_or_known_default().unwrap();
    let addresses = match url.host().unwrap() {
        Host::Ipv4(ip) => vec![SocketAddr::new(ip.into(), port)],
        Host::Ipv6(ip) => vec![SocketAddr::new(ip.into(), port)],
        Host::Domain(name) => tokio::time::timeout(
            Duration::from_secs(3),
            tokio::net::lookup_host((name, port)),
        )
        .await
        .map_err(|_| denied("DNS resolution timed out"))?
        .map_err(|_| denied("DNS resolution failed"))?
        .take(17)
        .collect(),
    };
    validate_addresses(&addresses)?;
    Ok(addresses)
}

fn client(url: &Url, addresses: &[SocketAddr]) -> Result<reqwest::Client, ToolFailure> {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .referer(false)
        .http1_only()
        .pool_max_idle_per_host(0)
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
        .timeout(TIMEOUT)
        .connect_timeout(Duration::from_secs(5))
        .resolve_to_addrs(url.host_str().unwrap(), addresses)
        .user_agent("Unified-AI-Bridge/public-get")
        .build()
        .map_err(|_| internal("HTTP client initialization failed"))
}

fn redirect(current: &Url, location: &str) -> Result<Url, ToolFailure> {
    let next = current
        .join(location)
        .map_err(|_| denied("invalid redirect destination"))?;
    let next = validate_url(next.as_str())?;
    if current.scheme() == "https" && next.scheme() != "https" {
        return Err(denied("HTTPS-to-HTTP redirects are denied"));
    }
    Ok(next)
}

async fn body(mut response: reqwest::Response, limit: usize) -> Result<Vec<u8>, ToolFailure> {
    if response
        .headers()
        .get(reqwest::header::CONTENT_ENCODING)
        .is_some_and(|v| v.as_bytes() != b"identity")
    {
        return Err(invalid(
            "compressed HTTP bodies are unsupported; response was not decompressed",
        ));
    }
    if response.content_length().is_some_and(|n| n > limit as u64) {
        return Err(invalid("HTTP response exceeds its byte limit"));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| internal("HTTP body read failed; request details withheld"))?
    {
        if chunk.len() > limit.saturating_sub(bytes.len()) {
            return Err(invalid("HTTP response exceeds its byte limit"));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

struct Received {
    bytes: Vec<u8>,
    content_type: String,
    origin: String,
    url_hash: String,
    redirects: usize,
}

async fn transfer(
    ctx: &ToolContext<'_>,
    p: &Policy,
    initial: &Url,
    writes: bool,
    limit: usize,
) -> Result<Received, ToolFailure> {
    let mut url = initial.clone();
    for hop in 0..=3 {
        check_policy(ctx, p, &url, writes)?;
        let addresses = resolve(&url).await?;
        check_policy(ctx, p, &url, writes)?;
        let origin = url.origin().ascii_serialization();
        let url_hash = sha256_bytes(url.as_str().as_bytes());
        ctx.app.audit_path_access(ctx.agent,ctx.session_id,"network.request",None,"allowed",&json!({"origin":origin,"url_sha256":url_hash,"hop":hop,"resolved_ips":addresses.iter().map(|a|a.ip().to_string()).collect::<Vec<_>>(),"method":"GET"}).to_string()).map_err(internal)?;
        let response = client(&url, &addresses)?
            .get(url.clone())
            .header(reqwest::header::ACCEPT_ENCODING, "identity")
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    internal("HTTP request timed out")
                } else {
                    internal("HTTP connection failed; request details withheld")
                }
            })?;
        if !response
            .remote_addr()
            .is_some_and(|remote| addresses.contains(&remote) && public_ip(remote.ip()))
        {
            return Err(denied(
                "HTTP peer did not match the pinned public DNS addresses",
            ));
        }
        let status = response.status();
        ctx.app.audit_path_access(ctx.agent,ctx.session_id,"network.response",None,"received",&json!({"origin":origin,"url_sha256":url_hash,"http_status":status.as_u16(),"hop":hop}).to_string()).map_err(internal)?;
        check_policy(ctx, p, &url, writes)?;
        if matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308) {
            if hop == 3 {
                return Err(denied("HTTP redirect limit exceeded (3)"));
            }
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| denied("redirect has no valid Location"))?;
            url = redirect(&url, location)?;
            continue;
        }
        if !status.is_success() {
            return Err(ToolFailure::Io(format!(
                "public server returned HTTP {}; no body saved",
                status.as_u16()
            )));
        }
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .filter(|v| v.len() <= 256)
            .unwrap_or("")
            .to_string();
        let bytes = body(response, limit).await?;
        check_policy(ctx, p, &url, writes)?;
        return Ok(Received {
            bytes,
            content_type,
            origin,
            url_hash,
            redirects: hop,
        });
    }
    unreachable!()
}

async fn reviewed(
    ctx: &ToolContext<'_>,
    tool: &str,
    raw: &str,
    path: Option<String>,
    limit: usize,
) -> Result<Received, ToolFailure> {
    let url = validate_url(raw)?;
    let p = policy(ctx.app);
    let writes = path.is_some();
    check_policy(ctx, &p, &url, writes)?;
    let _slot = SLOTS.try_acquire().map_err(|_| {
        invalid("four network operations are already pending; wait for one to finish")
    })?;
    let review=format!("Public anonymous GET: {}\nSave destination: {}\nLimit: {} bytes / 20 seconds / 3 redirects. Redirects may reach any explicitly granted origin: {}. No cookies, login, headers, proxy or automatic retry. This sends the URL to the remote server.",url,path.as_deref().unwrap_or("return bounded text only"),limit,p.origins.join(", "));
    tokio::select! {
        result=ctx.app.request_approval(ctx.session_id,ctx.agent,tool,&review,path)=>result?,
        error=wait_revoked(ctx,&p,&url,writes)=>return Err(error),
    }
    check_policy(ctx, &p, &url, writes)?;
    tokio::select! {
        result=tokio::time::timeout(TIMEOUT,transfer(ctx,&p,&url,writes,limit))=>result.map_err(|_|internal("network operation exceeded its 20-second deadline"))?,
        error=wait_revoked(ctx,&p,&url,writes)=>Err(error),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Fetch {
    url: String,
    #[serde(default = "text_limit")]
    max_bytes: usize,
}
fn text_limit() -> usize {
    12000
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Download {
    url: String,
    destination: String,
}

pub async fn fetch(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: Fetch = parse_args(args)?;
    if !(256..=24000).contains(&args.max_bytes) {
        return Err(invalid("max_bytes must be 256–24000"));
    }
    let r = reviewed(ctx, "fetch_url_text", &args.url, None, TEXT_BYTES).await?;
    let (output, truncated) = text::extract(&r.bytes, &r.content_type, args.max_bytes)?;
    let data = json!({"origin":r.origin,"url_sha256":r.url_hash,"body_sha256":sha256_bytes(&r.bytes),"received_bytes":r.bytes.len(),"text_bytes":output.len(),"truncated":truncated,"redirects":r.redirects,"content_type":r.content_type,"extraction":"static UTF-8 source text; no page rendering, CSS visibility or JavaScript"});
    ctx.app
        .audit_path_access(
            ctx.agent,
            ctx.session_id,
            "tool.fetch_url_text",
            None,
            "allowed",
            &data.to_string(),
        )
        .map_err(internal)?;
    Ok(ToolOutcome::read("Public web text", output, None).with_data(data))
}

pub async fn download(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: Download = parse_args(args)?;
    let initial = binary::target(ctx, "download_file", &args.destination)?;
    let p = policy(ctx.app);
    let r = reviewed(
        ctx,
        "download_file",
        &args.url,
        Some(initial.display()),
        FILE_BYTES,
    )
    .await?;
    let _lock =
        PathLock::acquire(&ctx.app.workspace.lock_dir, &initial.display()).map_err(internal)?;
    let target = binary::target(ctx, "download_file", &args.destination)?;
    check_policy(ctx, &p, &validate_url(&args.url)?, true)?;
    let data = json!({"origin":r.origin,"url_sha256":r.url_hash,"sha256":sha256_bytes(&r.bytes),"bytes":r.bytes.len(),"destination":target.display(),"redirects":r.redirects});
    let mut stage = binary::Stage::new(
        super::artifacts::directory(target.absolute.parent().unwrap())?,
        false,
    )?;
    let result = (|| {
        ctx.app
            .audit_path_access(
                ctx.agent,
                ctx.session_id,
                "download.prepare",
                Some(target.display()),
                "allowed",
                &data.to_string(),
            )
            .map_err(internal)?;
        stage.file.write_all(&r.bytes).map_err(internal)?;
        check_policy(ctx, &p, &validate_url(&args.url)?, true)?;
        binary::target(ctx, "download_file", &args.destination)?;
        stage.publish(&target.absolute)
    })();
    binary::staged_result(ctx, "download_file", &target.display(), &mut stage, result)?;
    ctx.app
        .audit_path_access(
            ctx.agent,
            ctx.session_id,
            "tool.download_file",
            Some(target.display()),
            "allowed",
            &data.to_string(),
        )
        .map_err(|_| {
            internal(
                "download published but final audit failed; inspect destination before retrying",
            )
        })?;
    Ok(ToolOutcome::write(
        "Public download saved",
        "Bytes saved in a new file; contents were not opened or executed.",
        Some(target.display()),
    )
    .with_data(data))
}

#[cfg(test)]
mod tests;
