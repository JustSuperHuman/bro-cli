//! Account-owned computers. Signing in alone never enables remote management (the monitoring service, the
//! desktop, commands). It does put the computer on the account for its person's own Bro sessions
//! ([`add_for_sessions`]), so the phone they sign in on lists it; nobody else on the account sees that kind.
pub mod health;

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    pub device_id: String,
    pub computer_id: String,
    pub origin: String,
    pub account_id: String,
    pub user_id: String,
    pub enabled: bool,
    #[serde(default)]
    pub selected: Option<String>,
    /// On the account for its person's own Bro sessions only: no monitoring service, no desktop, no commands.
    #[serde(default)]
    pub sessions_only: bool,
    /// The person switched off reaching this computer's sessions from their phone: signing in doesn't add it.
    #[serde(default)]
    pub sessions_off: bool,
}

impl Connection {
    /// Remote management (the monitoring service, the desktop, commands) is on, not just phone access to the
    /// sessions here.
    pub fn managed(&self) -> bool {
        self.enabled && !self.sessions_only && valid_id(&self.computer_id)
    }
    /// The sessions here can be opened from the person's phone (either kind of computer).
    pub fn reachable(&self) -> bool {
        self.enabled && valid_id(&self.computer_id)
    }
    /// Enrolled for remote management under some sign-in, switched on or not. Such a computer is never quietly
    /// made a sessions-only one.
    fn enrolled(&self) -> bool {
        valid_id(&self.computer_id) && !self.sessions_only
    }
}

pub fn path() -> PathBuf {
    crate::paths::bro_dir().join("jsh/justconnect.json")
}

/// Bro as a Windows service on a computer the RMM manages (`bro agent`): one backend for the whole computer, running
/// as its own account from boot, whether or not anyone is signed in to Windows. Once it is installed, the computer's
/// account connection is its job alone: a Bro a person opens on that computer leaves it be, so the computer never
/// answers twice.
pub mod agent {
    use serde::{Deserialize, Serialize};
    use std::path::PathBuf;

    /// The Windows service's name, and the account it runs as (`NT SERVICE\BroAgent`).
    pub const SERVICE: &str = "BroAgent";

    /// `%ProgramData%\Bro` (`BRO_PROGRAMDATA` overrides it, for tests).
    pub fn root() -> PathBuf {
        if let Some(dir) = std::env::var_os("BRO_PROGRAMDATA").filter(|d| !d.is_empty()) {
            return PathBuf::from(dir);
        }
        let base = std::env::var_os("ProgramData").filter(|d| !d.is_empty()).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
        base.join("Bro")
    }
    /// The agent's own `~/.bro` (its `BRO_DIR`).
    pub fn data_dir() -> PathBuf {
        root().join("agent")
    }
    pub fn marker_path() -> PathBuf {
        root().join("agent.json")
    }

    /// Written when the agent is installed, removed when it is uninstalled. Readable by everyone on the computer:
    /// it holds no secret, only which computer the agent answers for.
    #[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
    #[serde(rename_all = "camelCase")]
    pub struct Marker {
        pub computer_id: String,
        #[serde(default)]
        pub installed_at: i64,
        #[serde(default)]
        pub version: String,
        /// The Windows profile whose Just Superhuman sign-in the agent took over, if it took one.
        #[serde(default)]
        pub adopted_from: String,
    }

    pub fn installed() -> Option<Marker> {
        crate::util::read_json(&marker_path()).and_then(|v| serde_json::from_value(v).ok())
    }

    /// This process is the agent (the service starts it with `BRO_AGENT=1`).
    pub fn is_agent() -> bool {
        std::env::var("BRO_AGENT").is_ok_and(|v| v == "1")
    }

    /// The agent answers for computer `id` here, so this Bro must not.
    pub fn owns(id: &str) -> bool {
        !is_agent() && !id.is_empty() && installed().is_some_and(|m| m.computer_id == id)
    }

    /// The agent is installed here and this Bro isn't it: a Bro someone opens doesn't put the computer on the
    /// account by itself.
    pub fn elsewhere() -> bool {
        !is_agent() && installed().is_some()
    }
}
pub fn load() -> Connection {
    crate::util::read_json(&path())
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}
pub fn save(c: &Connection) -> anyhow::Result<()> {
    crate::util::atomic_write(&path(), serde_json::to_vec_pretty(c)?.as_slice())?;
    crate::creds::restrict_permissions(&path());
    Ok(())
}

pub fn valid_id(id: &str) -> bool {
    uuid::Uuid::parse_str(id).is_ok()
}

/// The longest the account server is asked to hold a waiting request (a worker's poll for work, a caller's
/// wait for a job's result), in seconds. Under the half minute after which proxies give up on a quiet request.
pub const WAIT_SECS: u64 = 25;

/// The account server answered with an error status: `message` is what it said, in its own words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    pub status: u16,
    pub message: String,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Refused {}

/// The status the account server refused a call with, when that's what the error is (None: the request never
/// got an answer, so nothing is known about what happened on the other side).
pub fn refused(error: &anyhow::Error) -> Option<&Refused> {
    error.downcast_ref::<Refused>()
}

/// Pauses between tries that start short and settle: a job that finishes in a moment is noticed in a moment,
/// one that takes a minute costs a request a second.
#[derive(Debug, Clone)]
pub struct Pauses {
    next: Duration,
    cap: Duration,
}

impl Pauses {
    pub fn new(first: Duration, cap: Duration) -> Pauses {
        Pauses { next: first, cap }
    }

    /// For following one job to its result.
    pub fn following() -> Pauses {
        Pauses::new(Duration::from_millis(80), Duration::from_secs(1))
    }

    /// The pause to take now; the one after it is half as long again, up to the cap.
    pub fn take(&mut self) -> Duration {
        let now = self.next;
        self.next = (self.next + self.next / 2).min(self.cap);
        now
    }
}

/// How long to stay away after `failures` tries in a row got no answer: doubling from a second to a minute, the
/// second half of each wait random so the computers of an account don't all come back in the same instant after
/// the server restarts. `roll` is a number in 0..1.
pub fn retry_after(failures: u32, roll: f64) -> Duration {
    let ceiling = Duration::from_secs(1u64 << failures.clamp(0, 6)).min(Duration::from_secs(60));
    ceiling / 2 + ceiling.mul_f64(roll.clamp(0.0, 1.0) / 2.0)
}

/// A number in 0..1 that differs between calls and between computers (no generator needed for spreading waits).
pub fn roll() -> f64 {
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&uuid::Uuid::new_v4().as_bytes()[..8]);
    (u64::from_le_bytes(bytes) >> 11) as f64 / (1u64 << 53) as f64
}

fn credentials() -> anyhow::Result<crate::jsh::Stored> {
    let stored = crate::jsh::stored().context("Sign in with `bro jsh login` first.")?;
    let url = url::Url::parse(&stored.origin)?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !(url.scheme() == "https"
            || url.scheme() == "http"
                && matches!(
                    url.host_str(),
                    Some("localhost" | "127.0.0.1" | "auth.localhost")
                ))
    {
        bail!("JustConnect requires an HTTPS account server.");
    }
    Ok(stored)
}

pub fn call(method: &str, route: &str, body: Value) -> anyhow::Result<Value> {
    if !route.starts_with('/') || route.contains("..") || route.contains(['?', '#', '\\']) {
        bail!("Invalid JustConnect route.");
    }
    let stored = credentials()?;
    call_with(&stored, method, route, body)
}

pub fn call_with(
    stored: &crate::jsh::Stored,
    method: &str,
    route: &str,
    body: Value,
) -> anyhow::Result<Value> {
    request(stored, method, route, body, None)
}

/// [`call_with`], willing to be kept waiting: the server may hold the request up to `wait` seconds for the thing
/// asked about to happen (work arriving for a worker's poll, a job finishing). A server that doesn't hold
/// requests answers at once, exactly as it always did; the caller paces its next try either way.
pub fn call_waiting(
    stored: &crate::jsh::Stored,
    method: &str,
    route: &str,
    body: Value,
    wait: u64,
) -> anyhow::Result<Value> {
    request(stored, method, route, body, Some(wait.clamp(1, WAIT_SECS)))
}

/// Where `route` lives on the account server that issued this sign-in.
pub fn endpoint(stored: &crate::jsh::Stored, route: &str) -> String {
    format!(
        "{}/api/justconnect/v1{route}",
        stored.origin.trim_end_matches('/')
    )
}

/// How long one call may take: the server's own limit for it, plus the round trip.
fn timeout(method: &str, route: &str, wait: Option<u64>) -> Duration {
    if let Some(wait) = wait {
        return Duration::from_secs(wait + 15);
    }
    Duration::from_secs(if route.ends_with("/installer") {
        250
    } else if route.ends_with("/poll")
        || route.contains("/results/")
        // handing work over, reading a job and listing computers touch only the account server's own records:
        // a slow answer is a broken connection, and waiting two minutes on it helps nobody
        || route.ends_with("/bro")
        || route.starts_with("/jobs/")
        || method == "GET" && (route == "/computers" || route == "/status")
    {
        20
    } else {
        140
    })
}

fn request(
    stored: &crate::jsh::Stored,
    method: &str,
    route: &str,
    mut body: Value,
    wait: Option<u64>,
) -> anyhow::Result<Value> {
    let url = endpoint(stored, route);
    // the process's shared connections: a job's submit, its result and the next poll ride one TLS session
    let agent = crate::http::shared();
    let limit = timeout(method, route, wait);
    let auth = format!("Bearer {}", stored.token);
    let response = match method {
        "GET" => {
            let mut call = agent
                .get(&url)
                .config()
                .timeout_global(Some(limit))
                .max_redirects(0)
                .build()
                .header("authorization", &auth);
            if let Some(wait) = wait {
                call = call.query("wait", wait.to_string());
            }
            call.call()?
        }
        "POST" => {
            if let (Some(wait), Some(fields)) = (wait, body.as_object_mut()) {
                fields.insert("wait".into(), json!(wait));
            }
            agent
                .post(&url)
                .config()
                .timeout_global(Some(limit))
                .max_redirects(0)
                .build()
                .header("authorization", &auth)
                .send_json(&body)?
        }
        _ => bail!("Unsupported JustConnect method."),
    };
    let status = response.status().as_u16();
    let data: Value = serde_json::from_str(&response.into_body().read_to_string()?)
        .context("JustConnect returned an invalid response.")?;
    if !(200..300).contains(&status) {
        return Err(Refused {
            status,
            message: data["error"]
                .as_str()
                .unwrap_or("JustConnect refused the request.")
                .to_string(),
        }
        .into());
    }
    Ok(data)
}

/// A live account check precedes enrollment. The local switch is set only after installation succeeds.
pub fn enable(name: Option<&str>) -> anyhow::Result<Value> {
    enable_with_claim(name, None)
}

/// A technician can supply a private, short-lived claim for an existing RMM agent.
pub fn enable_with_claim(name: Option<&str>, claim_file: Option<&Path>) -> anyhow::Result<Value> {
    let _enrollment = crate::rmm::lock(
        &crate::paths::bro_dir().join("jsh"),
        "justconnect-enrollment",
    )?;
    let stored = credentials()?;
    let account = match crate::jsh::fetch_account(&stored.origin, &stored.token)? {
        crate::jsh::Fetched::Account(a) => a,
        crate::jsh::Fetched::Revoked => {
            bail!("Your Just Superhuman sign-in expired. Run `bro jsh login`.")
        }
    };
    let mut c = load();
    if c.sessions_only && !same_sign_in(&c, &stored.origin, &account.organization.id, &account.user.id) {
        // a sessions-only computer of an earlier sign-in: nothing to transfer
        c.computer_id.clear();
    }
    if valid_id(&c.computer_id)
        && (c.origin != stored.origin
            || c.account_id != account.organization.id
            || c.user_id != account.user.id)
    {
        bail!(
            "This computer is enrolled under another Just Superhuman sign-in. Disable its remote management and remove its RMM agent before transferring ownership."
        );
    }
    if c.device_id.is_empty() {
        c.device_id = uuid::Uuid::new_v4().to_string();
    }
    c.enabled = false;
    c.origin = stored.origin.clone();
    c.account_id = account.organization.id.clone();
    c.user_id = account.user.id.clone();
    c.selected = None;
    save(&c)?;
    let (platform, arch) = platform_and_arch();
    let result = call_with(
        &stored,
        "POST",
        "/computers",
        json!({ "deviceId": c.device_id, "name": name.map(str::to_string).unwrap_or_else(crate::jsh::device_name), "platform": platform, "arch": arch }),
    )?;
    c.computer_id = result["computer"]["id"]
        .as_str()
        .filter(|id| valid_id(id))
        .context("JustConnect returned no computer ID.")?
        .into();
    save(&c)?;
    let activated: anyhow::Result<()> = (|| {
        let status = call_with(
            &stored,
            "GET",
            &format!("/computers/{}", c.computer_id),
            json!({}),
        )?;
        if status["computer"]["status"] == "awaiting_install" {
            let default_claim = crate::paths::bro_dir().join("jsh/rmm-claim.json");
            let claim_path = claim_file.unwrap_or(&default_claim);
            if claim_file.is_some() || claim_path.exists() {
                let claim = read_claim(claim_path, &account.organization.id)?;
                call_with(
                    &stored,
                    "POST",
                    &format!("/computers/{}/adopt", c.computer_id),
                    json!({"token":claim["token"]}),
                )?;
                let _ = std::fs::remove_file(claim_path);
            } else {
                if existing_rmm() {
                    bail!(
                        "An RMM agent is already installed. Ask your RMM administrator for a JustConnect claim, then use `bro computers enable --claim-file <file>`. The existing agent was kept."
                    );
                }
                let package = call_with(
                    &stored,
                    "POST",
                    &format!("/computers/{}/installer", c.computer_id),
                    json!({}),
                )?;
                install(&package["installer"])?;
            }
        }
        if crate::jsh::stored().as_ref() != Some(&stored) {
            bail!(
                "The Just Superhuman sign-in changed during installation. Enable remote management again."
            );
        }
        c.enabled = true;
        c.sessions_only = false;
        c.sessions_off = false;
        save(&c)?;
        Ok(())
    })();
    if let Err(error) = activated {
        // the account server made it an ordinary computer when it was asked to provision it
        c.sessions_only = false;
        c.enabled = false;
        let _ = save(&c);
        let _ = call_with(
            &stored,
            "POST",
            &format!("/computers/{}/disable", c.computer_id),
            json!({}),
        );
        return Err(error);
    }
    Ok(
        json!({ "computerId": c.computer_id, "enabled": true, "message": "Remote management enabled. Keep Bro running to accept Bro work. The RMM agent reports status after its first check-in." }),
    )
}

fn same_sign_in(c: &Connection, origin: &str, account_id: &str, user_id: &str) -> bool {
    c.origin == origin && c.account_id == account_id && c.user_id == user_id
}

fn platform_and_arch() -> (&'static str, &'static str) {
    let platform = if cfg!(target_os = "macos") {
        "darwin"
    } else {
        std::env::consts::OS
    };
    // The Windows MSI contains x86 and x64 agents; Windows ARM uses x64 emulation.
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" if cfg!(windows) => "amd64",
        "aarch64" => "arm64",
        "x86" => "386",
        "arm" => "arm",
        other => other,
    };
    (platform, arch)
}

/// Whether [`add_for_sessions`] has something to do for this sign-in: the computer isn't reachable from it yet,
/// the person hasn't switched that off, and it isn't enrolled for remote management (only the person changes
/// that, with its own switch).
pub fn wants_sessions(c: &Connection, origin: &str, account_id: &str, user_id: &str) -> bool {
    let same = same_sign_in(c, origin, account_id, user_id);
    if c.sessions_off && (same || c.origin.is_empty()) {
        return false;
    }
    if c.enrolled() {
        return false;
    }
    !(c.reachable() && same)
}

/// Put this computer on the signed-in person's account for their own Bro sessions, so the phone they sign in on
/// lists it and opens its sessions through the account relay. Installs nothing and asks nothing of remote
/// management; only that person reaches it. `Ok(None)` when there was nothing to do.
pub fn add_for_sessions() -> anyhow::Result<Option<Value>> {
    let _enrollment = crate::rmm::lock(
        &crate::paths::bro_dir().join("jsh"),
        "justconnect-enrollment",
    )?;
    let stored = credentials()?;
    let account = match crate::jsh::fetch_account(&stored.origin, &stored.token)? {
        crate::jsh::Fetched::Account(a) => a,
        crate::jsh::Fetched::Revoked => {
            bail!("Your Just Superhuman sign-in expired. Run `bro jsh login`.")
        }
    };
    let mut c = load();
    if !wants_sessions(&c, &stored.origin, &account.organization.id, &account.user.id) {
        return Ok(None);
    }
    if !same_sign_in(&c, &stored.origin, &account.organization.id, &account.user.id) {
        // another sign-in's sessions-only computer (or none yet): this sign-in gets one of its own
        c.computer_id.clear();
        c.sessions_off = false;
    }
    if c.device_id.is_empty() {
        c.device_id = uuid::Uuid::new_v4().to_string();
    }
    let (platform, arch) = platform_and_arch();
    let result = call_with(
        &stored,
        "POST",
        "/computers/sessions",
        json!({ "deviceId": c.device_id, "name": crate::jsh::device_name(), "platform": platform, "arch": arch }),
    )?;
    let id = result["computer"]["id"]
        .as_str()
        .filter(|id| valid_id(id))
        .context("JustConnect returned no computer ID.")?;
    if crate::jsh::stored().as_ref() != Some(&stored) {
        bail!("The Just Superhuman sign-in changed while adding this computer.");
    }
    c.computer_id = id.into();
    c.origin = stored.origin.clone();
    c.account_id = account.organization.id.clone();
    c.user_id = account.user.id.clone();
    c.sessions_only = result["computer"]["sessionsOnly"] != false;
    c.sessions_off = false;
    c.enabled = true;
    save(&c)?;
    Ok(Some(result))
}

/// Start or stop reaching this computer's sessions from the person's phone. Off sticks: signing in again
/// doesn't add it back. For a computer enrolled for remote management, off is the same as [`disable`].
pub fn set_sessions(on: bool) -> anyhow::Result<Value> {
    let mut c = load();
    c.sessions_off = !on;
    save(&c)?;
    if on {
        return Ok(add_for_sessions()?.unwrap_or_else(|| json!({ "enabled": load().enabled })));
    }
    disable()
}

fn read_claim(path: &Path, account_id: &str) -> anyhow::Result<Value> {
    let file = std::fs::File::open(path).context("Could not read the RMM claim file.")?;
    let mut text = String::new();
    file.take(4097).read_to_string(&mut text)?;
    if text.len() > 4096 {
        bail!("Invalid RMM claim file.");
    }
    let claim: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .context("Invalid RMM claim file.")?;
    if claim["accountId"] != account_id {
        bail!("This RMM claim belongs to a different Just Superhuman account.");
    }
    let token = claim["token"]
        .as_str()
        .context("The RMM claim has no token.")?;
    if token.len() != 64
        || !token
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    {
        bail!("Invalid RMM claim token.");
    }
    Ok(claim)
}

#[cfg(windows)]
fn existing_rmm() -> bool {
    use windows_sys::Win32::System::Registry::{
        HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY, RegCloseKey, RegOpenKeyExW,
    };
    let key: Vec<u16> = "SOFTWARE\\TacticalRMM\0".encode_utf16().collect();
    for view in [KEY_WOW64_64KEY, KEY_WOW64_32KEY] {
        let mut handle = std::ptr::null_mut();
        if unsafe {
            RegOpenKeyExW(
                HKEY_LOCAL_MACHINE,
                key.as_ptr(),
                0,
                KEY_READ | view,
                &mut handle,
            )
        } == 0
        {
            unsafe {
                RegCloseKey(handle);
            }
            return true;
        }
    }
    false
}

#[cfg(not(windows))]
fn existing_rmm() -> bool {
    [
        "/opt/tacticalagent/tacticalagent",
        "/usr/local/bin/tacticalagent",
        "/etc/tacticalagent",
    ]
    .iter()
    .any(|path| Path::new(path).exists())
}

pub fn disable() -> anyhow::Result<Value> {
    let _enrollment = crate::rmm::lock(
        &crate::paths::bro_dir().join("jsh"),
        "justconnect-enrollment",
    )?;
    let mut c = load();
    c.enabled = false;
    // a computer signing in put on the account would come straight back the next time Bro starts
    if c.sessions_only {
        c.sessions_off = true;
    }
    save(&c)?;
    if !valid_id(&c.computer_id) {
        return Ok(json!({"enabled":false}));
    }
    let stored = credentials()?;
    if stored.origin != c.origin {
        bail!(
            "Disabled Bro work locally. Sign in to the original account to disable its RMM access."
        );
    }
    call(
        "POST",
        &format!("/computers/{}/disable", c.computer_id),
        json!({}),
    )
}

pub fn select(id: &str) -> anyhow::Result<Value> {
    let mut c = load();
    if id == "local" {
        c.selected = None;
        save(&c)?;
        return Ok(json!({"selected":"local"}));
    }
    if !valid_id(id) {
        bail!("Choose the computer ID shown by `bro computers list`.");
    }
    // Bind the selection to the actual current sign-in, even on computers that are not enrolled themselves.
    let stored = credentials()?;
    let result = call_with(&stored, "GET", &format!("/computers/{id}"), json!({}))?;
    let account = match crate::jsh::fetch_account(&stored.origin, &stored.token)? {
        crate::jsh::Fetched::Account(a) => a,
        _ => bail!("Sign in again."),
    };
    c.selected = Some(id.into());
    if crate::jsh::stored().as_ref() != Some(&stored) {
        bail!("The sign-in changed. Select the computer again.");
    }
    // Keep enrollment ownership intact. Selection has its own account binding.
    let selection = json!({ "computerId": id, "origin": stored.origin, "userId": account.user.id, "accountId": account.organization.id, "credentialId": format!("{:x}", Sha256::digest(stored.token.as_bytes())) });
    crate::util::atomic_write(
        &crate::paths::bro_dir().join("jsh/computer-selection.json"),
        serde_json::to_vec(&selection)?.as_slice(),
    )?;
    save(&c)?;
    Ok(result)
}

pub fn selected() -> Option<String> {
    let c = load();
    let id = c.selected?;
    let stored = crate::jsh::stored()?;
    let selection =
        crate::util::read_json(&crate::paths::bro_dir().join("jsh/computer-selection.json"))?;
    (valid_id(&id)
        && selection["computerId"] == id
        && selection["origin"] == stored.origin
        && selection["credentialId"] == format!("{:x}", Sha256::digest(stored.token.as_bytes())))
    .then_some(id)
}

/// How long a caller follows a job before handing back whatever state it is in (delivery allows two minutes,
/// the work itself three).
const FOLLOW: Duration = Duration::from_secs(190);
/// Tries in a row that may get no answer before a caller gives up on a request it can safely repeat.
const TRIES: u32 = 4;

/// Run a Bro tool on computer `id` and wait for its result: the job as the server keeps it (`state`, `result`).
pub fn remote_tool(id: &str, tool: &str, arguments: Value) -> anyhow::Result<Value> {
    remote_tool_timed(id, tool, arguments).map(|(job, _)| job)
}

/// [`remote_tool`], with how long the round trip took (asking, the computer doing it, the answer coming back).
pub fn remote_tool_timed(id: &str, tool: &str, arguments: Value) -> anyhow::Result<(Value, Duration)> {
    if !valid_id(id) {
        bail!("Invalid computer ID.");
    }
    let stored = credentials()?;
    let start = std::time::Instant::now();
    // One request id for every try: the server hands back the job it already made for it, so a submit whose
    // answer was lost on the way is repeated without running the work twice.
    let ask = json!({"requestId":uuid::Uuid::new_v4().to_string(), "tool":tool,"arguments":arguments});
    let submitted = persist(|| call_with(&stored, "POST", &format!("/computers/{id}/bro"), ask.clone()))?;
    let job_id = submitted["job"]["id"]
        .as_str()
        .filter(|id| valid_id(id))
        .context("No job ID returned.")?
        .to_string();
    let route = format!("/jobs/{job_id}");
    let mut pauses = Pauses::following();
    let mut job = submitted["job"].clone();
    loop {
        if !matches!(job["state"].as_str(), Some("queued" | "leased")) || start.elapsed() >= FOLLOW {
            return Ok((job, start.elapsed()));
        }
        let left = FOLLOW.saturating_sub(start.elapsed()).as_secs().max(1);
        // a server that holds the request answers the moment the job settles; one that doesn't is asked again
        // after a pause that starts short
        let asked = std::time::Instant::now();
        let result = persist(|| call_waiting(&stored, "GET", &route, json!({}), left.min(WAIT_SECS)))?;
        job = result["job"].clone();
        if matches!(job["state"].as_str(), Some("queued" | "leased")) && asked.elapsed() < Duration::from_secs(1) {
            std::thread::sleep(pauses.take());
        }
    }
}

/// Try a request that is safe to repeat until it gets an answer (any answer: a refusal is one), a few times.
/// What comes back when none did is the last failure.
fn persist<T>(mut ask: impl FnMut() -> anyhow::Result<T>) -> anyhow::Result<T> {
    let mut failures = 0;
    loop {
        match ask() {
            Ok(answer) => return Ok(answer),
            Err(error) if refused(&error).is_some() => return Err(error),
            Err(error) => {
                failures += 1;
                if failures >= TRIES {
                    return Err(error);
                }
                std::thread::sleep(retry_after(failures - 1, roll()).min(Duration::from_secs(4)));
            }
        }
    }
}

fn install(package: &Value) -> anyhow::Result<()> {
    let url = url::Url::parse(package["url"].as_str().context("Missing installer URL.")?)?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
    {
        bail!("Invalid installer URL.");
    }
    let token = package["token"]
        .as_str()
        .context("Missing installer authorization.")?;
    let expected = package["sha256"]
        .as_str()
        .context("Missing installer checksum.")?;
    let size = package["size"]
        .as_u64()
        .filter(|n| *n > 0 && *n <= 256 * 1024 * 1024)
        .context("Invalid installer size.")?;
    let kind = package["kind"]
        .as_str()
        .context("Missing installer type.")?;
    if !matches!(
        (std::env::consts::OS, kind),
        ("windows", "msi") | ("linux", "sh") | ("macos", "bin")
    ) {
        bail!("Wrong installer for this platform.");
    }
    let root = crate::paths::bro_dir()
        .join("jsh/installers")
        .join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&root)?;
    crate::creds::restrict_permissions(&root);
    let target = root.join(format!("JustConnect.{kind}"));
    let result = (|| {
        let response = crate::http::shared()
            .get(url.as_str())
            .config()
            .timeout_global(Some(Duration::from_secs(180)))
            .max_redirects(0)
            .build()
            .header("authorization", format!("Bearer {token}"))
            .call()?;
        if response.status().as_u16() != 200 {
            bail!("The installer expired. Enable remote management again.");
        }
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)?;
        crate::creds::restrict_permissions(&target);
        let mut reader = response.into_body().into_reader().take(size + 1);
        let mut hasher = Sha256::new();
        let mut buf = [0u8; 65536];
        let mut total = 0u64;
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            total += n as u64;
            hasher.update(&buf[..n]);
            file.write_all(&buf[..n])?;
        }
        file.sync_all()?;
        drop(file);
        if total != size || format!("{:x}", hasher.finalize()) != expected {
            bail!("Installer checksum did not match. Nothing was installed.");
        }
        launch_installer(&target, kind, &package["arguments"])
    })();
    let _ = std::fs::remove_file(&target);
    let _ = std::fs::remove_dir(&root);
    result
}

#[cfg(windows)]
fn launch_installer(path: &Path, _: &str, _: &Value) -> anyhow::Result<()> {
    use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
    use windows::Win32::UI::Shell::{
        SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
    };
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::{HSTRING, PCWSTR};
    let verb = HSTRING::from("runas");
    let executable = crate::util::env_path("SystemRoot")
        .unwrap_or_else(|| PathBuf::from("C:\\Windows"))
        .join("System32/msiexec.exe");
    let file = HSTRING::from(executable.as_os_str());
    let parameters = HSTRING::from(format!("/i \"{}\" /passive /norestart", path.display()));
    unsafe {
        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
            lpVerb: PCWSTR(verb.as_ptr()),
            lpFile: PCWSTR(file.as_ptr()),
            lpParameters: PCWSTR(parameters.as_ptr()),
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        ShellExecuteExW(&mut info)
            .context("Windows did not approve the JustConnect installation.")?;
        let waited = WaitForSingleObject(info.hProcess, u32::MAX);
        let mut code = 1;
        let read = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hProcess);
        read?;
        if waited != WAIT_OBJECT_0 || !matches!(code, 0 | 3010) {
            bail!("JustConnect installation ended with Windows error {code}.");
        }
    }
    Ok(())
}

#[cfg(not(windows))]
fn launch_installer(path: &Path, kind: &str, arguments: &Value) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    let mut cmd = std::process::Command::new("sudo");
    if kind == "sh" {
        cmd.arg("/bin/bash");
    }
    cmd.env_remove("DISPLAY").env_remove("XAUTHORITY");
    cmd.arg(path);
    for argument in arguments
        .as_array()
        .context("Invalid installer arguments.")?
    {
        cmd.arg(argument.as_str().context("Invalid installer argument.")?);
    }
    let status = cmd.status()?;
    if !status.success() {
        bail!("JustConnect installation failed: {status}.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn the_agent_alone_answers_for_its_computer() {
        let temp = tempfile::tempdir().unwrap();
        // SAFETY: test-only environment, read back in this test
        unsafe { std::env::set_var("BRO_PROGRAMDATA", temp.path()) };
        const ID: &str = "dbb88f69-0da5-444f-a10c-9c17afcd48e0";
        assert!(!agent::owns(ID) && !agent::elsewhere(), "no agent installed");
        crate::util::atomic_write(&agent::marker_path(), serde_json::to_vec(&agent::Marker { computer_id: ID.into(), ..Default::default() }).unwrap().as_slice()).unwrap();
        assert!(agent::owns(ID));
        assert!(!agent::owns("another-computer"));
        assert!(agent::elsewhere());
        assert_eq!(agent::data_dir(), temp.path().join("agent"));
        unsafe { std::env::remove_var("BRO_PROGRAMDATA") };
    }
    #[test]
    fn signing_in_adds_the_computer_for_sessions_unless_switched_off_or_enrolled() {
        const ID: &str = "dbb88f69-0da5-444f-a10c-9c17afcd48e0";
        let me = |c: &Connection| wants_sessions(c, "https://jsh", "org", "me");
        // never on an account
        assert!(me(&Connection::default()));
        // already reachable from this sign-in
        let mine = Connection { computer_id: ID.into(), origin: "https://jsh".into(), account_id: "org".into(), user_id: "me".into(), enabled: true, sessions_only: true, ..Default::default() };
        assert!(!me(&mine));
        // signed out and back in: added again
        assert!(me(&Connection { enabled: false, ..mine.clone() }));
        // switched off by its person: stays off
        assert!(!me(&Connection { enabled: false, sessions_off: true, ..mine.clone() }));
        // someone else signs in here: they get their own
        assert!(wants_sessions(&Connection { enabled: false, sessions_off: true, ..mine.clone() }, "https://jsh", "org", "you"));
        // enrolled for remote management, on or off, under any sign-in: left to its own switch
        let enrolled = Connection { sessions_only: false, ..mine.clone() };
        assert!(!me(&enrolled));
        assert!(!me(&Connection { enabled: false, ..enrolled.clone() }));
        assert!(!wants_sessions(&enrolled, "https://jsh", "org", "you"));
        assert!(enrolled.managed() && !mine.managed() && mine.reachable());
    }
    #[test]
    fn computer_ids_cannot_escape_routes() {
        assert!(valid_id("dbb88f69-0da5-444f-a10c-9c17afcd48e0"));
        for id in ["../roles", "abc?accountId=other", "", "foo/bar"] {
            assert!(!valid_id(id));
        }
    }
    #[test]
    fn rmm_claim_must_match_the_signed_in_account() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("claim.json");
        std::fs::write(
            &path,
            serde_json::to_vec(&json!({"accountId":"alpha","token":"x".repeat(64)})).unwrap(),
        )
        .unwrap();
        assert!(read_claim(&path, "alpha").is_ok());
        assert!(read_claim(&path, "beta").is_err());
        std::fs::write(
            &path,
            serde_json::to_vec(&json!({"accountId":"alpha","token":"bad"})).unwrap(),
        )
        .unwrap();
        assert!(read_claim(&path, "alpha").is_err());
    }
    #[test]
    fn platform_checks_precede_download() {
        let package = json!({"url":"https://example.com/install", "token":"test", "sha256":"abc", "size":1, "kind":"wrong"});
        assert!(
            install(&package)
                .unwrap_err()
                .to_string()
                .contains("platform")
        );
        let package = json!({"url":"http://example.com/install"});
        assert!(install(&package).unwrap_err().to_string().contains("URL"));
    }
}
