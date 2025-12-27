//! Config + profile validation (friendly errors)
//!
//! Purpose:
//! - Catch common misconfigurations early
//! - Explain *what* is wrong, *where* it lives, and *what to do*
//! - Keep the engine running where possible by falling back safely

use std::collections::{BTreeSet, HashMap};

use crate::{loge, logw};

#[derive(Debug, Clone)]
pub struct ValidationIssue {
    pub level: IssueLevel,
    pub path: String,
    pub message: String,
    pub hint: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueLevel {
    Warn,
    Error,
}

impl ValidationIssue {
    pub fn warn(path: impl Into<String>, message: impl Into<String>, hint: Option<String>) -> Self {
        Self { level: IssueLevel::Warn, path: path.into(), message: message.into(), hint }
    }
    pub fn error(path: impl Into<String>, message: impl Into<String>, hint: Option<String>) -> Self {
        Self { level: IssueLevel::Error, path: path.into(), message: message.into(), hint }
    }
}

pub fn emit_issues(tag: &str, issues: &[ValidationIssue]) {
    for it in issues {
        match it.level {
            IssueLevel::Warn => {
                if let Some(h) = &it.hint {
                    logw!(tag, "{}: {} (hint: {})", it.path, it.message, h);
                } else {
                    logw!(tag, "{}: {}", it.path, it.message);
                }
            }
            IssueLevel::Error => {
                if let Some(h) = &it.hint {
                    loge!(tag, "{}: {} (hint: {})", it.path, it.message, h);
                } else {
                    loge!(tag, "{}: {}", it.path, it.message);
                }
            }
        }
    }
}


/// Emit a one-line summary even when there are zero issues.
/// This is helpful for auditability ("validation ran") and debugging.
pub fn emit_summary(tag: &str, label: &str, issues: &[ValidationIssue]) {
    let warns = issues.iter().filter(|i| i.level == IssueLevel::Warn).count();
    let errs = issues.iter().filter(|i| i.level == IssueLevel::Error).count();
    if errs == 0 && warns == 0 {
        crate::logi!(tag, "validation: {label} OK (0 issues)");
    } else {
        crate::logw!(tag, "validation: {label} issues found (errors={errs} warnings={warns})");
    }
}

/// Validate params.json profile relationships:
/// - duplicate param names
/// - profile uniform names exist in `params` list
/// - active profile names exist for each shader
pub fn validate_params_json(params: &serde_json::Value) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();

    // Collect canonical param names from params.params[*].name
    let mut names = Vec::new();
    if let Some(arr) = params.get("params").and_then(|v| v.as_array()) {
        for (i, p) in arr.iter().enumerate() {
            let base = format!("params.json:/params/{}", i);
            match p.get("name").and_then(|v| v.as_str()) {
                Some(n) => names.push(n.to_string()),
                None => issues.push(ValidationIssue::error(
                    format!("{base}/name"),
                    "missing or non-string param name",
                    Some("each entry in /params must include a string field 'name'".into()),
                )),
            }
        }
    } else {
        issues.push(ValidationIssue::error(
            "params.json:/params",
            "missing or non-array 'params'",
            Some("expected an array like: { \"params\": [ {\"name\": \"u_gain\", ...}, ... ] }".into()),
        ));
        return issues;
    }

    // Duplicate names
    let mut seen = BTreeSet::new();
    for n in &names {
        if !seen.insert(n.clone()) {
            issues.push(ValidationIssue::error(
                "params.json:/params",
                format!("duplicate param name '{n}'"),
                Some("param names must be unique; duplicates make mappings ambiguous".into()),
            ));
        }
    }

    let name_set: BTreeSet<String> = names.iter().cloned().collect();

    // shader_profiles[shader_path][profile_name].uniforms keys should exist in params list
    if let Some(shader_profiles) = params.get("shader_profiles").and_then(|v| v.as_object()) {
        for (shader_path, profiles_v) in shader_profiles.iter() {
            let shader_base = format!("params.json:/shader_profiles/{}", escape_ptr(shader_path));
            let profiles = match profiles_v.as_object() {
                Some(o) => o,
                None => {
                    issues.push(ValidationIssue::error(
                        shader_base,
                        "shader_profiles entry must be an object of profiles",
                        Some("expected: { \"default\": {\"uniforms\": {...}}, \"lofi\": {...} }".into()),
                    ));
                    continue;
                }
            };

            if !profiles.contains_key("default") {
                issues.push(ValidationIssue::warn(
                    shader_base.clone(),
                    "no 'default' profile found for this shader",
                    Some("recommended to include a 'default' profile for predictable startup".into()),
                ));
            }

            for (profile_name, prof_v) in profiles.iter() {
                let prof_base = format!("{shader_base}/{}", escape_ptr(profile_name));
                let uniforms = prof_v.get("uniforms").and_then(|u| u.as_object());
                match uniforms {
                    Some(uo) => {
                        for (uname, _) in uo.iter() {
                            if !name_set.contains(uname) {
                                issues.push(ValidationIssue::warn(
                                    format!("{prof_base}/uniforms/{}", escape_ptr(uname)),
                                    format!("uniform '{uname}' not declared in params.json:/params"),
                                    Some("add it under /params, or remove it from this profile".into()),
                                ));
                            }
                        }
                    }
                    None => {
                        issues.push(ValidationIssue::warn(
                            format!("{prof_base}/uniforms"),
                            "missing or non-object 'uniforms' for this profile",
                            Some("expected: \"uniforms\": { \"u_gain\": 0.5, ... }".into()),
                        ));
                    }
                }
            }
        }
    } else {
        issues.push(ValidationIssue::warn(
            "params.json:/shader_profiles",
            "missing or non-object 'shader_profiles'",
            Some("profiles are optional; omit if you don't need per-shader defaults".into()),
        ));
    }

    // active_shader_profiles should reference existing shader_profiles entries + existing profile name
    let mut shader_to_profiles: HashMap<String, BTreeSet<String>> = HashMap::new();
    if let Some(shader_profiles) = params.get("shader_profiles").and_then(|v| v.as_object()) {
        for (shader, profs_v) in shader_profiles {
            if let Some(profs) = profs_v.as_object() {
                shader_to_profiles.insert(shader.clone(), profs.keys().cloned().collect());
            }
        }
    }

    if let Some(active) = params.get("active_shader_profiles").and_then(|v| v.as_object()) {
        for (shader, prof_name_v) in active.iter() {
            let prof_name = prof_name_v.as_str().unwrap_or("");
            let path = format!("params.json:/active_shader_profiles/{}", escape_ptr(shader));

            if prof_name.is_empty() {
                issues.push(ValidationIssue::warn(
                    path,
                    "active profile name is empty or non-string",
                    Some("set to a valid profile name, e.g. \"default\"".into()),
                ));
                continue;
            }

            match shader_to_profiles.get(shader) {
                None => issues.push(ValidationIssue::warn(
                    path,
                    format!("active profile '{prof_name}' references shader '{shader}' with no entry under shader_profiles"),
                    Some("either add shader_profiles[shader] or remove this active_shader_profiles entry".into()),
                )),
                Some(set) => {
                    if !set.contains(prof_name) {
                        issues.push(ValidationIssue::warn(
                            path,
                            format!("active profile '{prof_name}' not found; available: {}", join_set(set)),
                            Some("fix the name, or add that profile under shader_profiles for this shader".into()),
                        ));
                    }
                }
            }
        }
    } else {
        issues.push(ValidationIssue::warn(
            "params.json:/active_shader_profiles",
            "missing or non-object 'active_shader_profiles'",
            Some("optional; if omitted, ShadeCore falls back to 'default' when available".into()),
        ));
    }

    issues
}

/// Validate recording config linkage:
/// - recording.json.active_profile exists in recording.profiles.json
pub fn validate_recording_profiles(rec_cfg: &serde_json::Value, rec_profiles: &serde_json::Value) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();

    let active = rec_cfg.get("active_profile").and_then(|v| v.as_str()).unwrap_or("");
    let profiles = rec_profiles.get("profiles").and_then(|v| v.as_object());

    if profiles.is_none() {
        issues.push(ValidationIssue::error(
            "recording.profiles.json:/profiles",
            "missing or non-object 'profiles'",
            Some("expected: { \"profiles\": { \"1080p_prores\": {...}, ... } }".into()),
        ));
        return issues;
    }
    let profiles = profiles.unwrap();

    if active.is_empty() {
        let avail: BTreeSet<String> = profiles.keys().cloned().collect();
        issues.push(ValidationIssue::warn(
            "recording.json:/active_profile",
            "missing or empty active_profile",
            Some(format!("set to one of: {}", join_set(&avail))),
        ));
    } else if !profiles.contains_key(active) {
        let avail: BTreeSet<String> = profiles.keys().cloned().collect();
        issues.push(ValidationIssue::warn(
            "recording.json:/active_profile",
            format!("active_profile '{active}' not found"),
            Some(format!("available: {}", join_set(&avail))),
        ));
    }

    issues
}

fn join_set(set: &BTreeSet<String>) -> String {
    let mut v: Vec<_> = set.iter().cloned().collect();
    v.sort();
    v.join(", ")
}

// JSON Pointer escaping for friendly paths
fn escape_ptr(s: &str) -> String {
    s.replace('~', "~0").replace('/', "~1")
}
