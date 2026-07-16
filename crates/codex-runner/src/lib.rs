#![deny(unsafe_code)]
#![deny(clippy::all)]
#![deny(unreachable_pub)]
#![warn(missing_docs)]

//! Codex host runner on Unicity AOS.
//!
//! Host adapter: bounded `codex exec` turns under AOS. The first
//! slice uses bounded `codex exec` calls rather than pretending Codex has the
//! same long-lived stdin/stdout contract as `claude -p`.

use astrid_sdk::prelude::*;
use oracle_host::ids::stamped_principal;
use oracle_host::ids::validate_id as host_validate_id;
// InteractionMode re-exported above
use serde::{Deserialize, Serialize};
use std::time::UNIX_EPOCH;

mod provision;

const SETTINGS_KEY: &str = "codex.principal.config";
const SESSION_KEY_PREFIX: &str = "codex.session";
const HOOK_TOKEN_KEY_PREFIX: &str = "codex.hook_token";
const MAX_CODEX_EVENT_LINES: usize = 512;

/// Shared headless/repl axis.
pub(crate) use oracle_host::InteractionMode;

/// Per-principal Codex runner settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrincipalConfig {
    /// Headless or REPL mode.
    #[serde(default)]
    pub interaction_mode: InteractionMode,
    /// Codex approval policy.
    #[serde(default = "default_approval_policy")]
    pub approval_policy: String,
    /// Codex sandbox mode.
    #[serde(default = "default_sandbox_mode")]
    pub sandbox_mode: String,
    /// Optional model override.
    #[serde(default)]
    pub model: Option<String>,
    /// Codex config profile to layer over the base config.
    #[serde(default = "default_profile")]
    pub profile: Option<String>,
    /// Run Codex without persisting session files.
    #[serde(default)]
    pub ephemeral: bool,
    /// Ignore base user config for bounded automation turns.
    #[serde(default)]
    pub ignore_user_config: bool,
    /// Ignore user/project execpolicy rule files.
    #[serde(default)]
    pub ignore_rules: bool,
    /// Permit execution outside a Git repository.
    #[serde(default)]
    pub skip_git_repo_check: bool,
    /// Mirror `codex exec --json` JSONL records onto Astrid events.
    #[serde(default = "default_mirror_json_events")]
    pub mirror_json_events: bool,
    /// Wire-format version.
    #[serde(default = "PrincipalConfig::default_schema_version")]
    pub schema_version: u32,
}

impl Default for PrincipalConfig {
    fn default() -> Self {
        Self {
            interaction_mode: InteractionMode::Headless,
            approval_policy: default_approval_policy(),
            sandbox_mode: default_sandbox_mode(),
            model: None,
            profile: default_profile(),
            ephemeral: false,
            ignore_user_config: false,
            ignore_rules: false,
            skip_git_repo_check: false,
            mirror_json_events: default_mirror_json_events(),
            schema_version: Self::SCHEMA_VERSION,
        }
    }
}

impl PrincipalConfig {
    /// Current config schema version.
    pub const SCHEMA_VERSION: u32 = 1;

    fn default_schema_version() -> u32 {
        Self::SCHEMA_VERSION
    }

    fn validate(&self) -> Result<(), SysError> {
        if self.schema_version > Self::SCHEMA_VERSION {
            return Err(SysError::ApiError(format!(
                "Codex config schema_version {} exceeds supported {}",
                self.schema_version,
                Self::SCHEMA_VERSION
            )));
        }
        validate_approval_policy(&self.approval_policy)?;
        validate_sandbox_mode(&self.sandbox_mode)?;
        if let Some(profile) = self.profile.as_deref() {
            validate_topic_segment("profile", profile)?;
        }
        Ok(())
    }
}

fn default_approval_policy() -> String {
    "on-request".to_string()
}

fn default_sandbox_mode() -> String {
    "workspace-write".to_string()
}

fn default_profile() -> Option<String> {
    Some("aos".to_string())
}

fn default_mirror_json_events() -> bool {
    true
}

fn default_config_from_env() -> Result<PrincipalConfig, SysError> {
    let mut cfg = PrincipalConfig::default();

    if let Some(mode) = env_value("interaction_mode") {
        cfg.interaction_mode = match mode.as_str() {
            "headless" => InteractionMode::Headless,
            "repl" => InteractionMode::Repl,
            _ => {
                return Err(SysError::ApiError(format!(
                    "unsupported Codex interaction_mode '{mode}'"
                )));
            }
        };
    }
    if let Some(policy) = env_value("approval_policy") {
        cfg.approval_policy = policy;
    }
    if let Some(sandbox) = env_value("sandbox_mode") {
        cfg.sandbox_mode = sandbox;
    }
    if let Some(model) = env_value("model") {
        cfg.model = Some(model);
    }
    if let Some(profile) = env_value("profile") {
        cfg.profile = Some(profile);
    }
    if let Some(ephemeral) = env_bool("ephemeral")? {
        cfg.ephemeral = ephemeral;
    }
    if let Some(ignore_user_config) = env_bool("ignore_user_config")? {
        cfg.ignore_user_config = ignore_user_config;
    }
    if let Some(ignore_rules) = env_bool("ignore_rules")? {
        cfg.ignore_rules = ignore_rules;
    }
    if let Some(skip_git_repo_check) = env_bool("skip_git_repo_check")? {
        cfg.skip_git_repo_check = skip_git_repo_check;
    }
    if let Some(mirror_json_events) = env_bool("mirror_json_events")? {
        cfg.mirror_json_events = mirror_json_events;
    }

    cfg.validate()?;
    Ok(cfg)
}

fn env_value(key: &str) -> Option<String> {
    env::var_opt(key)
        .ok()
        .flatten()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_bool(key: &str) -> Result<Option<bool>, SysError> {
    let Some(value) = env_value(key) else {
        return Ok(None);
    };
    parse_bool_env_value(value.as_str())
        .map(Some)
        .ok_or_else(|| SysError::ApiError(format!("{key} must be true or false, got '{value}'")))
}

fn parse_bool_env_value(value: &str) -> Option<bool> {
    match value {
        "true" | "1" | "yes" | "on" => Some(true),
        "false" | "0" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn load_config() -> Result<PrincipalConfig, SysError> {
    let cfg = match kv::get_json_opt::<PrincipalConfig>(SETTINGS_KEY)? {
        Some(cfg) => cfg,
        None => default_config_from_env()?,
    };
    cfg.validate()?;
    Ok(cfg)
}

fn save_config(cfg: &PrincipalConfig) -> Result<(), SysError> {
    cfg.validate()?;
    kv::set_json(SETTINGS_KEY, cfg)
}

/// Runner capsule singleton.
#[derive(Default)]
pub struct CodexRunner;

/// `codex.v1.request.spawn` payload.
#[derive(Debug, Deserialize)]
pub struct SpawnRequest {
    /// Astrid principal this turn belongs to.
    pub principal_id: String,
    /// Optional caller-provided session id; generated when absent.
    #[serde(default)]
    pub session_id: Option<String>,
    /// Optional workspace/mount identifier for attribution.
    #[serde(default)]
    pub workspace_id: Option<String>,
    /// Optional current working directory observed by the launcher.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Optional first prompt.
    #[serde(default)]
    pub initial_message: Option<String>,
}

/// `codex.v1.request.send.<sid>` payload.
#[derive(Debug, Deserialize)]
pub struct SendRequest {
    /// Target session id.
    pub session_id: String,
    /// Prompt body.
    pub text: String,
}

/// `codex.v1.hook.<sid>.<event>` payload emitted by the Codex plugin shim.
#[derive(Debug, Deserialize)]
pub struct HookEnvelope {
    /// Wire-format version.
    #[serde(default)]
    pub schema_version: u32,
    /// Claimed Astrid principal.
    pub principal_id: String,
    /// Claimed Codex session.
    pub session_id: String,
    /// Hook event name.
    pub event: String,
    /// Optional workspace/mount identifier.
    #[serde(default)]
    pub workspace_id: Option<String>,
    /// Optional process id from the native hook process.
    #[serde(default)]
    pub pid: Option<u32>,
    /// Optional parent process id from the native hook process.
    #[serde(default)]
    pub ppid: Option<u32>,
    /// Base64-encoded raw Codex hook payload.
    #[serde(default)]
    pub payload_b64: Option<String>,
    /// Base64-encoded current working directory observed by the hook shim.
    #[serde(default)]
    pub cwd_b64: Option<String>,
    /// Claimed per-session hook token.
    #[serde(default)]
    pub token: Option<String>,
}

/// `codex.v1.request.settings.set` payload.
#[derive(Debug, Deserialize)]
pub struct SettingsSetRequest {
    /// Principal whose settings are being changed.
    pub principal_id: String,
    /// Optional interaction mode patch.
    #[serde(default)]
    pub interaction_mode: Option<InteractionMode>,
    /// Optional approval policy patch.
    #[serde(default)]
    pub approval_policy: Option<String>,
    /// Optional sandbox mode patch.
    #[serde(default)]
    pub sandbox_mode: Option<String>,
    /// Optional model patch. Empty string clears the override.
    #[serde(default)]
    pub model: Option<String>,
    /// Optional Codex profile patch. Empty string clears the override.
    #[serde(default)]
    pub profile: Option<String>,
    /// Optional `codex exec --ephemeral` patch.
    #[serde(default)]
    pub ephemeral: Option<bool>,
    /// Optional `codex exec --ignore-user-config` patch.
    #[serde(default)]
    pub ignore_user_config: Option<bool>,
    /// Optional `codex exec --ignore-rules` patch.
    #[serde(default)]
    pub ignore_rules: Option<bool>,
    /// Optional `codex exec --skip-git-repo-check` patch.
    #[serde(default)]
    pub skip_git_repo_check: Option<bool>,
    /// Optional JSONL event mirroring patch.
    #[serde(default)]
    pub mirror_json_events: Option<bool>,
}

#[derive(Debug, Serialize)]
struct Spawned {
    principal_id: String,
    session_id: String,
    workspace_id: Option<String>,
    mode: &'static str,
}

#[derive(Debug, Serialize)]
struct TurnResult {
    principal_id: String,
    session_id: String,
    workspace_id: Option<String>,
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
    codex_events: CodexEventSummary,
}

#[derive(Debug, Clone, Default, Serialize)]
struct CodexEventSummary {
    seen: usize,
    published: usize,
    malformed: usize,
    truncated: bool,
}

#[derive(Debug, Serialize)]
struct MirroredCodexEvent {
    principal_id: String,
    session_id: String,
    workspace_id: Option<String>,
    sequence: usize,
    event_type: String,
    event: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct CodexEventParseError {
    principal_id: String,
    session_id: String,
    workspace_id: Option<String>,
    sequence: usize,
    reason: String,
    line: String,
}

#[derive(Debug, Serialize)]
struct ErrorEvent {
    principal_id: Option<String>,
    session_id: Option<String>,
    reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SessionStatus {
    Active,
    Finished,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AgentSession {
    schema_version: u32,
    principal_id: String,
    agent_kind: String,
    session_id: String,
    workspace_id: Option<String>,
    cwd: Option<String>,
    started_at_ms: u128,
    last_seen_at_ms: u128,
    status: SessionStatus,
}

#[derive(Debug, Serialize)]
struct HookAudit {
    principal_id: String,
    session_id: String,
    event: String,
    workspace_id: Option<String>,
    pid: Option<u32>,
    ppid: Option<u32>,
    verified: bool,
    reason: Option<String>,
}

#[capsule]
impl CodexRunner {
    /// Spawn a Codex headless turn.
    #[astrid::interceptor("handle_spawn")]
    pub fn handle_spawn(&self, mut req: SpawnRequest) -> Result<(), SysError> {
        req.principal_id = stamped_principal(&req.principal_id)?.to_string();
        validate_principal_id("principal_id", &req.principal_id)?;
        let session_id = req
            .session_id
            .unwrap_or_else(|| generated_segment_id("codex-session"));
        validate_topic_segment("session_id", &session_id)?;
        if let Some(workspace_id) = req.workspace_id.as_deref() {
            validate_topic_segment("workspace_id", workspace_id)?;
        }

        let cfg = load_config()?;
        if cfg.interaction_mode == InteractionMode::Repl {
            publish_error(
                Some(&req.principal_id),
                Some(&session_id),
                "interaction_mode_is_repl: run Codex directly for this principal",
            )?;
            return Ok(());
        }

        if let Err(reason) = provision::ensure(&req.principal_id) {
            publish_error(
                Some(&req.principal_id),
                Some(&session_id),
                &format!("install_failed: {reason}"),
            )?;
            return Ok(());
        }

        let session = register_session(
            &req.principal_id,
            &session_id,
            req.workspace_id.clone(),
            req.cwd.clone(),
        )?;

        ipc::publish_json(
            &format!("codex.v1.event.{session_id}.spawned"),
            &Spawned {
                principal_id: req.principal_id.clone(),
                session_id: session_id.clone(),
                workspace_id: req.workspace_id.clone(),
                mode: "headless",
            },
        )?;

        if let Some(prompt) = req.initial_message {
            run_codex_turn(&session, &cfg, &prompt)?;
        }
        Ok(())
    }

    /// Send one prompt as a bounded Codex exec turn.
    #[astrid::interceptor("handle_send")]
    pub fn handle_send(&self, req: SendRequest) -> Result<(), SysError> {
        validate_topic_segment("session_id", &req.session_id)?;
        let cfg = load_config()?;
        let Some(mut session) = load_session(&req.session_id)? else {
            publish_error(None, Some(&req.session_id), "unknown_session")?;
            return Ok(());
        };
        stamped_principal(&session.principal_id)?;
        if cfg.interaction_mode == InteractionMode::Repl {
            publish_error(
                Some(&session.principal_id),
                Some(&req.session_id),
                "interaction_mode_is_repl: run Codex directly for this principal",
            )?;
            return Ok(());
        }
        touch_session(&mut session, SessionStatus::Active)?;
        run_codex_turn(&session, &cfg, &req.text)
    }

    /// Patch per-principal Codex settings.
    #[astrid::interceptor("handle_settings_set")]
    pub fn handle_settings_set(&self, mut req: SettingsSetRequest) -> Result<(), SysError> {
        req.principal_id = stamped_principal(&req.principal_id)?.to_string();
        validate_principal_id("principal_id", &req.principal_id)?;
        let mut cfg = load_config()?;
        if let Some(mode) = req.interaction_mode {
            cfg.interaction_mode = mode;
        }
        if let Some(policy) = req.approval_policy {
            validate_approval_policy(&policy)?;
            cfg.approval_policy = policy;
        }
        if let Some(sandbox) = req.sandbox_mode {
            validate_sandbox_mode(&sandbox)?;
            cfg.sandbox_mode = sandbox;
        }
        if let Some(model) = req.model {
            let trimmed = model.trim();
            cfg.model = (!trimmed.is_empty()).then(|| trimmed.to_string());
        }
        if let Some(profile) = req.profile {
            let trimmed = profile.trim();
            if trimmed.is_empty() {
                cfg.profile = None;
            } else {
                validate_topic_segment("profile", trimmed)?;
                cfg.profile = Some(trimmed.to_string());
            }
        }
        if let Some(ephemeral) = req.ephemeral {
            cfg.ephemeral = ephemeral;
        }
        if let Some(ignore_user_config) = req.ignore_user_config {
            cfg.ignore_user_config = ignore_user_config;
        }
        if let Some(ignore_rules) = req.ignore_rules {
            cfg.ignore_rules = ignore_rules;
        }
        if let Some(skip_git_repo_check) = req.skip_git_repo_check {
            cfg.skip_git_repo_check = skip_git_repo_check;
        }
        if let Some(mirror_json_events) = req.mirror_json_events {
            cfg.mirror_json_events = mirror_json_events;
        }
        save_config(&cfg)?;
        ipc::publish_json("codex.v1.settings.changed", &cfg)
    }

    /// Record a Codex hook emitted by the native/plugin shim.
    #[astrid::interceptor("handle_hook")]
    pub fn handle_hook(&self, mut env: HookEnvelope) -> Result<(), SysError> {
        env.principal_id = stamped_principal(&env.principal_id)?.to_string();
        validate_principal_id("principal_id", &env.principal_id)?;
        validate_topic_segment("session_id", &env.session_id)?;
        validate_topic_segment("event", &env.event)?;
        if let Some(workspace_id) = env.workspace_id.as_deref() {
            validate_topic_segment("workspace_id", workspace_id)?;
        }

        let mut session = load_session(&env.session_id)?.unwrap_or_else(|| AgentSession {
            schema_version: 1,
            principal_id: env.principal_id.clone(),
            agent_kind: "codex".to_string(),
            session_id: env.session_id.clone(),
            workspace_id: env.workspace_id.clone(),
            cwd: None,
            started_at_ms: now_millis(),
            last_seen_at_ms: now_millis(),
            status: SessionStatus::Active,
        });
        session.workspace_id = session.workspace_id.or_else(|| env.workspace_id.clone());
        touch_session(&mut session, SessionStatus::Active)?;

        let (verified, reason) = verify_hook_token(&env)?;
        ipc::publish_json(
            "codex.v1.audit.hook_received",
            &HookAudit {
                principal_id: env.principal_id,
                session_id: env.session_id,
                event: env.event,
                workspace_id: env.workspace_id,
                pid: env.pid,
                ppid: env.ppid,
                verified,
                reason,
            },
        )
    }
}

fn run_codex_turn(
    session: &AgentSession,
    cfg: &PrincipalConfig,
    prompt: &str,
) -> Result<(), SysError> {
    let token = mint_token()?;
    persist_hook_token(&session.principal_id, &session.session_id, &token)?;

    let mut cmd = process::Command::new("codex")
        .arg("exec")
        .arg("--json")
        .arg("--ask-for-approval")
        .arg(cfg.approval_policy.clone())
        .arg("--sandbox")
        .arg(cfg.sandbox_mode.clone())
        .env("ASTRID_PRINCIPAL_ID", session.principal_id.clone())
        .env("ASTRID_SESSION_ID", session.session_id.clone())
        .env("ASTRID_HOOK_TOKEN", token);

    if let Some(workspace_id) = session.workspace_id.as_deref() {
        cmd = cmd.env("ASTRID_WORKSPACE_ID", workspace_id);
    }

    if let Some(cwd) = session.cwd.as_deref().filter(|cwd| !cwd.trim().is_empty()) {
        cmd = cmd.arg("--cd").arg(cwd);
    }

    if let Some(model) = cfg.model.as_deref().filter(|m| !m.trim().is_empty()) {
        cmd = cmd.arg("--model").arg(model);
    }
    if let Some(profile) = cfg.profile.as_deref().filter(|p| !p.trim().is_empty()) {
        cmd = cmd.arg("--profile").arg(profile);
    }
    if cfg.ephemeral {
        cmd = cmd.arg("--ephemeral");
    }
    if cfg.ignore_user_config {
        cmd = cmd.arg("--ignore-user-config");
    }
    if cfg.ignore_rules {
        cmd = cmd.arg("--ignore-rules");
    }
    if cfg.skip_git_repo_check {
        cmd = cmd.arg("--skip-git-repo-check");
    }

    let output = cmd.arg(prompt).spawn()?;
    let codex_events = if cfg.mirror_json_events {
        mirror_codex_json_events(session, &output.stdout)?
    } else {
        CodexEventSummary::default()
    };
    let event = TurnResult {
        principal_id: session.principal_id.clone(),
        session_id: session.session_id.clone(),
        workspace_id: session.workspace_id.clone(),
        exit_code: output.exit.exit_code,
        stdout: output.stdout,
        stderr: output.stderr,
        codex_events,
    };
    let topic = if output.exit.success() {
        mark_session_status(&session.session_id, SessionStatus::Finished)?;
        format!("codex.v1.event.{}.done", session.session_id)
    } else {
        mark_session_status(&session.session_id, SessionStatus::Error)?;
        format!("codex.v1.event.{}.error", session.session_id)
    };
    ipc::publish_json(&topic, &event)
}

fn mirror_codex_json_events(
    session: &AgentSession,
    stdout: &str,
) -> Result<CodexEventSummary, SysError> {
    let mut summary = CodexEventSummary::default();
    for (idx, line) in stdout.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        summary.seen += 1;
        let sequence = idx + 1;
        if summary.published >= MAX_CODEX_EVENT_LINES {
            summary.truncated = true;
            continue;
        }
        match serde_json::from_str::<serde_json::Value>(trimmed) {
            Ok(event) => {
                let event_type = codex_event_type(&event);
                let topic_segment = codex_event_topic_segment(&event_type);
                ipc::publish_json(
                    &format!(
                        "codex.v1.event.{}.codex.{}",
                        session.session_id, topic_segment
                    ),
                    &MirroredCodexEvent {
                        principal_id: session.principal_id.clone(),
                        session_id: session.session_id.clone(),
                        workspace_id: session.workspace_id.clone(),
                        sequence,
                        event_type,
                        event,
                    },
                )?;
                summary.published += 1;
            }
            Err(err) => {
                summary.malformed += 1;
                ipc::publish_json(
                    "codex.v1.audit.codex_json_event_malformed",
                    &CodexEventParseError {
                        principal_id: session.principal_id.clone(),
                        session_id: session.session_id.clone(),
                        workspace_id: session.workspace_id.clone(),
                        sequence,
                        reason: err.to_string(),
                        line: clamp_for_audit(trimmed),
                    },
                )?;
            }
        }
    }
    Ok(summary)
}

fn codex_event_type(event: &serde_json::Value) -> String {
    event
        .get("type")
        .and_then(serde_json::Value::as_str)
        .filter(|event_type| !event_type.trim().is_empty())
        .unwrap_or("unknown")
        .to_string()
}

fn codex_event_topic_segment(event_type: &str) -> String {
    let mut out = String::with_capacity(event_type.len());
    let mut last_dash = false;
    for c in event_type.chars() {
        let valid = c.is_ascii_alphanumeric() || c == '_' || c == '-';
        if valid {
            out.push(c);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "unknown".to_string()
    } else {
        trimmed.to_string()
    }
}

fn clamp_for_audit(value: &str) -> String {
    const MAX: usize = 512;
    if value.len() <= MAX {
        value.to_string()
    } else {
        let mut out = value.chars().take(MAX).collect::<String>();
        out.push_str("...");
        out
    }
}

fn publish_error(
    principal_id: Option<&str>,
    session_id: Option<&str>,
    reason: &str,
) -> Result<(), SysError> {
    let topic = session_id
        .map(|sid| format!("codex.v1.event.{sid}.error"))
        .unwrap_or_else(|| "codex.v1.event.session_rejected".to_string());
    ipc::publish_json(
        &topic,
        &ErrorEvent {
            principal_id: principal_id.map(ToOwned::to_owned),
            session_id: session_id.map(ToOwned::to_owned),
            reason: reason.to_string(),
        },
    )
}

fn validate_principal_id(field: &str, id: &str) -> Result<(), SysError> {
    host_validate_id(field, id)
}

fn validate_topic_segment(field: &str, id: &str) -> Result<(), SysError> {
    validate_principal_id(field, id)?;
    if id.contains('.') {
        return Err(SysError::ApiError(format!(
            "{field} must be one IPC topic segment and cannot contain '.'"
        )));
    }
    Ok(())
}

fn validate_approval_policy(value: &str) -> Result<(), SysError> {
    match value {
        "untrusted" | "on-failure" | "on-request" | "never" => Ok(()),
        _ => Err(SysError::ApiError(format!(
            "unsupported Codex approval_policy '{value}'"
        ))),
    }
}

fn validate_sandbox_mode(value: &str) -> Result<(), SysError> {
    match value {
        "read-only" | "workspace-write" | "danger-full-access" => Ok(()),
        _ => Err(SysError::ApiError(format!(
            "unsupported Codex sandbox_mode '{value}'"
        ))),
    }
}

fn generated_segment_id(prefix: &str) -> String {
    match runtime::random_bytes(16) {
        Ok(bytes) => format!("{prefix}-{}", hex_encode(&bytes)),
        Err(_) => format!("{prefix}-{}", now_millis()),
    }
}

fn register_session(
    principal_id: &str,
    session_id: &str,
    workspace_id: Option<String>,
    cwd: Option<String>,
) -> Result<AgentSession, SysError> {
    let now = now_millis();
    let session = AgentSession {
        schema_version: 1,
        principal_id: principal_id.to_string(),
        agent_kind: "codex".to_string(),
        session_id: session_id.to_string(),
        workspace_id,
        cwd,
        started_at_ms: now,
        last_seen_at_ms: now,
        status: SessionStatus::Active,
    };
    save_session(&session)?;
    Ok(session)
}

fn load_session(session_id: &str) -> Result<Option<AgentSession>, SysError> {
    kv::get_json_opt(&session_key(session_id))
}

fn save_session(session: &AgentSession) -> Result<(), SysError> {
    kv::set_json(&session_key(&session.session_id), session)
}

fn touch_session(session: &mut AgentSession, status: SessionStatus) -> Result<(), SysError> {
    session.last_seen_at_ms = now_millis();
    session.status = status;
    save_session(session)
}

fn mark_session_status(session_id: &str, status: SessionStatus) -> Result<(), SysError> {
    if let Some(mut session) = load_session(session_id)? {
        touch_session(&mut session, status)?;
    }
    Ok(())
}

fn session_key(session_id: &str) -> String {
    format!("{SESSION_KEY_PREFIX}.{session_id}")
}

fn hook_token_key(principal_id: &str, session_id: &str) -> String {
    format!("{HOOK_TOKEN_KEY_PREFIX}.{principal_id}.{session_id}")
}

fn mint_token() -> Result<String, SysError> {
    runtime::random_bytes(32).map(|bytes| hex_encode(&bytes))
}

fn persist_hook_token(principal_id: &str, session_id: &str, token: &str) -> Result<(), SysError> {
    kv::set_bytes(&hook_token_key(principal_id, session_id), token.as_bytes())
}

fn verify_hook_token(env: &HookEnvelope) -> Result<(bool, Option<String>), SysError> {
    let Some(expected) = kv::get_bytes_opt(&hook_token_key(&env.principal_id, &env.session_id))?
    else {
        if env.event == "session_start"
            && let Some(token) = env.token.as_deref().filter(|t| !t.is_empty())
        {
            persist_hook_token(&env.principal_id, &env.session_id, token)?;
            return Ok((true, Some("registered_session_token".to_string())));
        }
        return Ok((false, Some("no_registered_token".to_string())));
    };
    let Some(token) = env.token.as_deref().filter(|t| !t.is_empty()) else {
        return Ok((false, Some("missing_token".to_string())));
    };
    if expected == token.as_bytes() {
        Ok((true, None))
    } else {
        Ok((false, Some("token_mismatch".to_string())))
    }
}

fn now_millis() -> u128 {
    time::now()
        .and_then(|now| {
            now.duration_since(UNIX_EPOCH)
                .map_err(|err| SysError::ApiError(err.to_string()))
        })
        .map(|dur| dur.as_millis())
        .unwrap_or(0)
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from_digit(u32::from(b >> 4), 16).unwrap_or('0'));
        s.push(char::from_digit(u32::from(b & 0x0F), 16).unwrap_or('0'));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn codex_event_type_defaults_to_unknown() {
        assert_eq!(codex_event_type(&json!({})), "unknown");
        assert_eq!(codex_event_type(&json!({ "type": "" })), "unknown");
        assert_eq!(
            codex_event_type(&json!({ "type": "turn.started" })),
            "turn.started"
        );
    }

    #[test]
    fn codex_event_topic_segment_rejects_topic_smuggling() {
        assert_eq!(codex_event_topic_segment("turn.started"), "turn-started");
        assert_eq!(
            codex_event_topic_segment("../item.completed"),
            "item-completed"
        );
        assert_eq!(codex_event_topic_segment(""), "unknown");
    }

    #[test]
    fn clamp_for_audit_is_char_boundary_safe() {
        let value = "é".repeat(600);
        let clamped = clamp_for_audit(&value);
        assert!(clamped.ends_with("..."));
        assert_eq!(clamped.chars().count(), 515);
    }

    #[test]
    fn config_defaults_to_aos_profile_and_event_mirroring() {
        let cfg = PrincipalConfig::default();
        assert_eq!(cfg.profile.as_deref(), Some("aos"));
        assert!(cfg.mirror_json_events);
        assert!(!cfg.ignore_user_config);
    }

    #[test]
    fn bool_env_parser_accepts_operator_spellings() {
        assert_eq!(parse_bool_env_value("true"), Some(true));
        assert_eq!(parse_bool_env_value("1"), Some(true));
        assert_eq!(parse_bool_env_value("yes"), Some(true));
        assert_eq!(parse_bool_env_value("on"), Some(true));
        assert_eq!(parse_bool_env_value("false"), Some(false));
        assert_eq!(parse_bool_env_value("0"), Some(false));
        assert_eq!(parse_bool_env_value("no"), Some(false));
        assert_eq!(parse_bool_env_value("off"), Some(false));
        assert_eq!(parse_bool_env_value("maybe"), None);
    }
}
