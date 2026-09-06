use anyhow::Error;
use serde_json::{Map, Value, json};

pub const CONTRACT_VERSION: &str = "clash.machine/v0";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    CliArgumentInvalid,
    ExplicitInputRequired,
    RuntimeConfigRequired,
    UnsupportedMachineAction,
    ProfileNotFound,
    ProfileAlreadyExists,
    ProfileNotReady,
    ProfileInvalid,
    StateConflict,
    PermissionRequired,
    NetworkError,
    ChecksumMismatch,
    ServiceOperationFailed,
    ConfigInvalid,
    FileIoError,
    UnsupportedPlatform,
    NotFound,
    InternalError,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CliArgumentInvalid => "CLI_ARGUMENT_INVALID",
            Self::ExplicitInputRequired => "EXPLICIT_INPUT_REQUIRED",
            Self::RuntimeConfigRequired => "RUNTIME_CONFIG_REQUIRED",
            Self::UnsupportedMachineAction => "UNSUPPORTED_MACHINE_ACTION",
            Self::ProfileNotFound => "PROFILE_NOT_FOUND",
            Self::ProfileAlreadyExists => "PROFILE_ALREADY_EXISTS",
            Self::ProfileNotReady => "PROFILE_NOT_READY",
            Self::ProfileInvalid => "PROFILE_INVALID",
            Self::StateConflict => "STATE_CONFLICT",
            Self::PermissionRequired => "PERMISSION_REQUIRED",
            Self::NetworkError => "NETWORK_ERROR",
            Self::ChecksumMismatch => "CHECKSUM_MISMATCH",
            Self::ServiceOperationFailed => "SERVICE_OPERATION_FAILED",
            Self::ConfigInvalid => "CONFIG_INVALID",
            Self::FileIoError => "FILE_IO_ERROR",
            Self::UnsupportedPlatform => "UNSUPPORTED_PLATFORM",
            Self::NotFound => "NOT_FOUND",
            Self::InternalError => "INTERNAL_ERROR",
        }
    }

    pub fn retryable(self) -> bool {
        matches!(self, Self::NetworkError | Self::ServiceOperationFailed)
    }

    pub fn state_changed(self) -> Option<bool> {
        match self {
            Self::CliArgumentInvalid
            | Self::ExplicitInputRequired
            | Self::RuntimeConfigRequired
            | Self::UnsupportedMachineAction
            | Self::ProfileNotFound
            | Self::ProfileAlreadyExists
            | Self::ProfileNotReady
            | Self::ProfileInvalid
            | Self::StateConflict
            | Self::PermissionRequired
            | Self::ChecksumMismatch
            | Self::ConfigInvalid
            | Self::UnsupportedPlatform
            | Self::NotFound => Some(false),
            Self::NetworkError
            | Self::FileIoError
            | Self::ServiceOperationFailed
            | Self::InternalError => None,
        }
    }
}

#[derive(Debug)]
pub struct CodedError {
    code: ErrorCode,
    message: String,
}

impl std::fmt::Display for CodedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CodedError {}

pub fn coded_error(code: ErrorCode, message: impl Into<String>) -> Error {
    CodedError {
        code,
        message: message.into(),
    }
    .into()
}

pub fn classify_error(err: &Error) -> ErrorCode {
    if let Some(coded) = err
        .chain()
        .find_map(|cause| cause.downcast_ref::<CodedError>())
    {
        return coded.code;
    }
    if err
        .chain()
        .any(|cause| cause.downcast_ref::<reqwest::Error>().is_some())
    {
        return ErrorCode::NetworkError;
    }
    if err
        .chain()
        .any(|cause| cause.downcast_ref::<std::io::Error>().is_some())
    {
        return ErrorCode::FileIoError;
    }

    // 旧的 anyhow 调用点会逐步迁移到 CodedError；这里作为 v0 兜底，避免未知错误伪装成成功。
    let message = err.to_string().to_lowercase();
    if message.contains("profile 不存在") || message.contains("订阅不存在") {
        return ErrorCode::ProfileNotFound;
    }
    if message.contains("权限不足") || message.contains("需要 root") {
        return ErrorCode::PermissionRequired;
    }
    if message.contains("sha256") {
        return ErrorCode::ChecksumMismatch;
    }
    if message.contains("systemctl") || message.contains("launchctl") || message.contains("服务")
    {
        return ErrorCode::ServiceOperationFailed;
    }
    if message.contains("当前仅支持") || message.contains("暂不支持") {
        return ErrorCode::UnsupportedPlatform;
    }
    if message.contains("不存在") || message.contains("未找到") {
        return ErrorCode::NotFound;
    }
    ErrorCode::InternalError
}

#[derive(Debug, Clone, Copy)]
pub struct ActionSemantics {
    pub mutating: bool,
}

impl ActionSemantics {
    pub const READ: Self = Self { mutating: false };
    pub const WRITE: Self = Self { mutating: true };
}

fn execution_status(payload: &Value) -> &'static str {
    let restart_failed = payload
        .get("restart_attempted")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        && payload
            .get("restart_ok")
            .is_some_and(|v| v == &Value::Bool(false));
    let explicit_partial = payload
        .get("partial")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if restart_failed || explicit_partial {
        "partial"
    } else {
        "success"
    }
}

fn normalized_data(payload: &Value) -> Value {
    let Some(object) = payload.as_object() else {
        return payload.clone();
    };
    let mut data = Map::new();
    for (key, value) in object {
        if key != "ok" && key != "action" {
            data.insert(key.clone(), value.clone());
        }
    }
    Value::Object(data)
}

fn successful_state_changed(payload: &Value, semantics: ActionSemantics) -> bool {
    if !semantics.mutating {
        return false;
    }
    if payload.get("skipped").and_then(Value::as_bool) == Some(true)
        || payload.get("updated").and_then(Value::as_bool) == Some(false)
        || payload.get("changed").and_then(Value::as_bool) == Some(false)
    {
        return false;
    }
    true
}

fn meta() -> Value {
    json!({
        "cli_version": env!("CARGO_PKG_VERSION"),
        "platform": std::env::consts::OS,
        "arch": std::env::consts::ARCH
    })
}

pub fn success_envelope(
    payload: &Value,
    canonical_action: &str,
    semantics: ActionSemantics,
) -> Value {
    let status = execution_status(payload);
    let changed = successful_state_changed(payload, semantics);
    json!({
        "contract": CONTRACT_VERSION,
        "ok": true,
        "status": status,
        "action": canonical_action,
        "effect": {
            "state_changed": changed,
            "verified": !semantics.mutating
        },
        "data": normalized_data(payload),
        "error": Value::Null,
        "meta": meta()
    })
}

pub fn error_envelope(err: &Error, canonical_action: &str) -> Value {
    let code = classify_error(err);
    json!({
        "contract": CONTRACT_VERSION,
        "ok": false,
        "status": "failed",
        "action": canonical_action,
        "effect": {
            "state_changed": code.state_changed(),
            "verified": false
        },
        "data": Value::Null,
        "error": {
            "code": code.as_str(),
            "message": err.to_string(),
            "retryable": code.retryable()
        },
        "meta": meta()
    })
}

pub fn contract_description() -> Value {
    let action = |name: &str, mutating: bool| {
        json!({
            "name": name,
            "mutating": mutating,
            "verified_on_success": !mutating
        })
    };
    json!({
        "version": CONTRACT_VERSION,
        "activation": "explicit --machine only",
        "envelope": {
            "required": ["contract", "ok", "status", "action", "effect", "data", "error", "meta"],
            "status": ["success", "partial", "failed"],
            "effect": {
                "state_changed": "boolean on successful actions; null when a failed action may have partially changed external state",
                "verified": "true only when the command is observational/read-only or explicitly post-verified"
            }
        },
        "actions": [
            action("contract.describe", false),
            action("ui.install", true),
            action("ui.status", false),
            action("ui.url", false),
            action("mode.get", false),
            action("mode.set", true),
            action("sub.add", true),
            action("sub.list", false),
            action("sub.fetch", true),
            action("sub.remove", true),
            action("sub.render", true),
            action("sub.validate", false),
            action("sub.mixin.show", false),
            action("sub.mixin.set", true),
            action("sub.mixin.unset", true),
            action("sub.mixin.reset", true),
            action("proxy.list", false),
            action("proxy.switch", true),
            action("system.on", true),
            action("system.off", true),
            action("system.status", false),
            action("env.on", false),
            action("env.off", false),
            action("tun.doctor", false),
            action("tun.on", true),
            action("tun.off", true),
            action("tun.status", false),
            action("core.install", true),
            action("core.upgrade", true),
            action("core.version", false),
            action("core.path", false),
            action("service.install", true),
            action("service.uninstall", true),
            action("service.enable", true),
            action("service.disable", true),
            action("service.start", true),
            action("service.stop", true),
            action("service.restart", true),
            action("service.status", false),
            action("service.log", false),
            action("api.status", false),
            action("api.connections", false),
            action("api.rules", false),
            action("api.configs", false),
            action("api.providers", false),
            action("api.close-connections", true),
            action("api.config-patch", true),
            action("api.traffic", false),
            action("api.logs", false),
            action("update.check", false),
            action("update.run", true)
        ],
        "human_only": [
            "sub.update",
            "sub add --fetch",
            "sub.use",
            "setup.init",
            "setup.unify",
            "ui.open",
            "service.log --follow",
            "proxy start/stop/status/auto"
        ],
        "error_codes": [
            "CLI_ARGUMENT_INVALID",
            "EXPLICIT_INPUT_REQUIRED",
            "RUNTIME_CONFIG_REQUIRED",
            "UNSUPPORTED_MACHINE_ACTION",
            "PROFILE_NOT_FOUND",
            "PROFILE_ALREADY_EXISTS",
            "PROFILE_NOT_READY",
            "PROFILE_INVALID",
            "STATE_CONFLICT",
            "PERMISSION_REQUIRED",
            "NETWORK_ERROR",
            "CHECKSUM_MISMATCH",
            "SERVICE_OPERATION_FAILED",
            "CONFIG_INVALID",
            "FILE_IO_ERROR",
            "UNSUPPORTED_PLATFORM",
            "NOT_FOUND",
            "INTERNAL_ERROR"
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;

    #[test]
    fn wraps_payload_under_canonical_action() {
        let envelope = success_envelope(
            &json!({"ok": true, "action": "ignored", "active": "main"}),
            "sub.list",
            ActionSemantics::READ,
        );
        assert_eq!(envelope["contract"], CONTRACT_VERSION);
        assert_eq!(envelope["action"], "sub.list");
        assert_eq!(envelope["data"]["active"], "main");
        assert_eq!(envelope["effect"]["state_changed"], false);
        assert_eq!(envelope["effect"]["verified"], true);
        assert!(envelope["data"].get("ok").is_none());
        assert!(envelope["data"].get("action").is_none());
    }

    #[test]
    fn mutation_is_explicitly_unverified_until_observed() {
        let envelope = success_envelope(&json!({"ok": true}), "sub.render", ActionSemantics::WRITE);
        assert_eq!(envelope["effect"]["state_changed"], true);
        assert_eq!(envelope["effect"]["verified"], false);
    }

    #[test]
    fn restart_failure_is_partial() {
        let envelope = success_envelope(
            &json!({
                "restart_attempted": true,
                "restart_ok": false
            }),
            "tun.on",
            ActionSemantics::WRITE,
        );
        assert_eq!(envelope["status"], "partial");
    }

    #[test]
    fn contract_description_has_unique_non_overlapping_actions() {
        use std::collections::HashSet;

        let description = contract_description();
        let actions = description["actions"].as_array().expect("actions array");
        let names: Vec<&str> = actions
            .iter()
            .map(|item| item["name"].as_str().expect("action name"))
            .collect();
        let unique: HashSet<&str> = names.iter().copied().collect();
        assert_eq!(names.len(), unique.len(), "Machine action 不得重复");

        let human_only = description["human_only"]
            .as_array()
            .expect("human_only array");
        for item in human_only {
            let name = item.as_str().expect("human-only name");
            assert!(
                !unique.contains(name),
                "同一动作不能同时属于 machine 与 human-only: {name}"
            );
        }
    }

    #[test]
    fn coded_error_beats_message_guessing() {
        let err = coded_error(ErrorCode::StateConflict, "任意文案");
        assert_eq!(classify_error(&err), ErrorCode::StateConflict);
        let fallback = anyhow!("profile 不存在: demo");
        assert_eq!(classify_error(&fallback), ErrorCode::ProfileNotFound);
    }
}
