//! Node version preflight: confirm the runtime that will execute `dsh web`
//! satisfies upstream's `engines.node` before we spawn the supervisor.
//!
//! Upstream dsh does not perform a runtime Node-version check; on an
//! unsupported runtime it prints the readiness URL line and then crashes
//! when a downstream plugin or native binding trips on a missing `node:*`
//! API. The only programmatic contract is the `engines.node` field in the
//! root `package.json` (currently `"^22.19.0 || >=24.0.0"`). This module
//! reads that contract and probes the local toolchain against it.
//!
//! Pure parsing lives here so it can be unit-tested without spawning any
//! process. The actual `Command::spawn` calls live in [`run_probe`], which
//! the caller is expected to invoke from inside a tokio runtime.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::Serialize;
use tokio::process::Command;

/// Upstream's hard runtime requirement (mirrors `H:\code\deepseek-harness\package.json`).
pub const REQUIRED_ENGINE: &str = "^22.19.0 || >=24.0.0";

/// What binary the probed version came from.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum VersionSource {
    /// First token of `DSH_CLIENT_BIN` — the node binary that will actually
    /// run `dsh web` because it overrides `dsh`/`npx` on PATH.
    OverrideBin,
    /// First `node` found on PATH (the fallback if no override is set).
    PathNode,
    /// Neither produced a usable version string.
    Unavailable,
}

/// Snapshot of the local toolchain's fitness for running `dsh web`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreflightReport {
    /// True iff the probed Node satisfies [`REQUIRED_ENGINE`] AND `dsh`
    /// itself was spawnable from PATH. The UI gates the supervisor on this.
    pub engine_ok: bool,
    /// Raw `node -p "process.version"` output, e.g. `"v22.19.0"`. `None`
    /// when probing failed.
    pub version: Option<String>,
    /// Which binary the version came from.
    pub version_source: VersionSource,
    /// Absolute path of the first `node` on PATH, for diagnostics — nvm shims
    /// can lie about which `node.exe` actually runs.
    pub node_path: Option<String>,
    /// The engine spec we tested against (always the value of
    /// [`REQUIRED_ENGINE`], surfaced for the UI).
    pub required: String,
    /// Whether spawning the planned `dsh` command resolved to a runnable
    /// binary (PATH has `dsh` or `npx`).
    pub dsh_reachable: bool,
    /// Localized failure summary the UI shows above the steps block.
    pub failure: Option<String>,
}

/// Parse `^x.y.z` / `>=x.y.z` clauses joined by `||`. Whitespace around
/// clauses and operators is tolerated. Empty input or an unparsable clause
/// yields `false` rather than `true` — fail closed.
///
/// We hand-roll this rather than pulling in the `semver` crate because the
/// shape we need is tiny: caret ranges, `>=`, and OR.
pub fn engine_satisfies(version: &str, spec: &str) -> bool {
    let parsed = match parse_version(version) {
        Some(v) => v,
        None => return false,
    };
    for clause in split_clauses(spec) {
        if clause_satisfies(&parsed, clause.trim()) {
            return true;
        }
    }
    false
}

fn split_clauses(spec: &str) -> impl Iterator<Item = &str> {
    spec.split("||")
}

fn clause_satisfies(v: &Version, clause: &str) -> bool {
    let clause = clause.trim();
    if clause.is_empty() {
        return false;
    }
    if let Some(rest) = clause.strip_prefix(">=") {
        return match parse_version(rest.trim()) {
            Some(min) => compare(v, &min) != std::cmp::Ordering::Less,
            None => false,
        };
    }
    if let Some(rest) = clause.strip_prefix(">") {
        return match parse_version(rest.trim()) {
            Some(min) => compare(v, &min) == std::cmp::Ordering::Greater,
            None => false,
        };
    }
    if let Some(rest) = clause.strip_prefix("^") {
        // npm caret: same major, minor/patch >= bound. `^22.19.0` accepts
        // 22.19.0..23.0.0 exclusive on the major bump.
        let min = match parse_version(rest.trim()) {
            Some(v) => v,
            None => return false,
        };
        if v.major != min.major {
            return false;
        }
        return compare(v, &min) != std::cmp::Ordering::Less;
    }
    // Bare x.y.z is treated as exact equality (npm accepts this for engines).
    match parse_version(clause) {
        Some(target) => compare(v, &target) == std::cmp::Ordering::Equal,
        None => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Version {
    major: u32,
    minor: u32,
    patch: u32,
}

fn parse_version(raw: &str) -> Option<Version> {
    // Tolerate a leading "v" and any pre-release / build suffix we don't
    // model: only the numeric triplet matters for `engines`.
    let trimmed = raw.trim().trim_start_matches('v');
    // Walk the string greedily, collecting digits and dots. Stop at the
    // first non-digit/non-dot; that gives us the full "x.y.z" run.
    let end = trimmed
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(trimmed.len());
    let head = &trimmed[..end];
    if head.is_empty() {
        return None;
    }
    let mut parts = head.split('.').filter(|p| !p.is_empty());
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    Some(Version { major, minor, patch })
}

fn compare(a: &Version, b: &Version) -> std::cmp::Ordering {
    a.major
        .cmp(&b.major)
        .then(a.minor.cmp(&b.minor))
        .then(a.patch.cmp(&b.patch))
}

/// Probe the toolchain and return a [`PreflightReport`]. Caller is expected
/// to be inside a tokio runtime. Spawning `node -p "process.version"` is
/// bounded by [`PROBE_TIMEOUT`] so a hung binary cannot stall startup.
pub async fn run_probe(
    bin_override_first_token: Option<&str>,
    path_node: Option<&Path>,
    dsh_program: &str,
) -> PreflightReport {
    let node_path = path_node.map(|p| p.to_string_lossy().into_owned());

    // Prefer the override's first token — it is the binary that will
    // actually run `dsh web`. Fall back to PATH `node` if no override is set.
    let (probe_target, version_source) = match bin_override_first_token {
        Some(bin) if !bin.trim().is_empty() => (Some(PathBuf::from(bin)), VersionSource::OverrideBin),
        _ => match path_node {
            Some(p) => (Some(p.to_path_buf()), VersionSource::PathNode),
            None => (None, VersionSource::Unavailable),
        },
    };

    let version = match probe_target.as_ref() {
        Some(bin) => probe_version(bin).await,
        None => None,
    };

    let engine_ok = match (&version, version_source) {
        (Some(v), _) => engine_satisfies(v, REQUIRED_ENGINE),
        _ => false,
    };

    let dsh_reachable = dsh_program != "dsh" || crate::resolve::find_in_path("dsh").is_some()
        || crate::resolve::find_in_path("npx").is_some();

    let failure = if !engine_ok {
        Some(build_failure_message(version.as_deref(), version_source))
    } else if !dsh_reachable {
        Some("无法在 PATH 中找到 dsh 或 npx；请安装 @deepseek-ai/dsh 或确保 npx 可用".to_string())
    } else {
        None
    };

    PreflightReport {
        engine_ok: engine_ok && dsh_reachable,
        version,
        version_source,
        node_path,
        required: REQUIRED_ENGINE.to_string(),
        dsh_reachable,
        failure,
    }
}

const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

async fn probe_version(bin: &Path) -> Option<String> {
    let mut cmd = build_probe_command(bin);
    cmd.arg("-p")
        .arg("process.version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = cmd.spawn().ok()?;
    let output = match tokio::time::timeout(PROBE_TIMEOUT, child.wait_with_output()).await {
        Ok(Ok(out)) => out,
        _ => return None,
    };
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() {
        None
    } else {
        Some(stdout)
    }
}

fn build_probe_command(bin: &Path) -> Command {
    // Windows .cmd shims must be run through cmd.exe (mirrors resolve.rs).
    #[cfg(windows)]
    {
        let mut c = Command::new("cmd");
        c.arg("/c").arg(bin);
        c
    }
    #[cfg(unix)]
    {
        Command::new(bin)
    }
}

fn build_failure_message(version: Option<&str>, source: VersionSource) -> String {
    match (version, source) {
        (Some(v), _) => format!(
            "检测到 Node {v}，不满足上游要求 {REQUIRED_ENGINE}；请用 nvm-windows 切换到 22.19+ 或 24+ 后重启应用"
        ),
        (None, VersionSource::Unavailable) => format!(
            "未在 PATH 中找到 node；请先安装 Node 22.19+ 或 24+，或设置 DSH_CLIENT_BIN 指向已有的 node.exe"
        ),
        (None, _) => format!(
            "虽然找到了 node 二进制，但无法读取其版本；请确认 node 可正常执行（`node -v`），要求 {REQUIRED_ENGINE}"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caret_rejects_one_below_minor() {
        assert!(!engine_satisfies("v22.18.5", "^22.19.0"));
        assert!(!engine_satisfies("22.18.99", "^22.19.0"));
    }

    #[test]
    fn caret_rejects_major_bump() {
        assert!(!engine_satisfies("v23.0.0", "^22.19.0"));
        assert!(!engine_satisfies("v24.0.0", "^22.19.0"));
    }

    #[test]
    fn caret_accepts_minor_within_same_major() {
        assert!(engine_satisfies("v22.19.0", "^22.19.0"));
        assert!(engine_satisfies("v22.99.3", "^22.19.0"));
    }

    #[test]
    fn gte_accepts_open_ended_upper() {
        assert!(engine_satisfies("v24.0.0", ">=24.0.0"));
        assert!(engine_satisfies("v25.7.1", ">=24.0.0"));
        assert!(!engine_satisfies("v23.99.0", ">=24.0.0"));
    }

    #[test]
    fn or_clause_combines() {
        assert!(engine_satisfies("v22.19.0", "^22.19.0 || >=24.0.0"));
        assert!(engine_satisfies("v22.99.0", "^22.19.0 || >=24.0.0"));
        assert!(!engine_satisfies("v23.5.0", "^22.19.0 || >=24.0.0"));
        assert!(engine_satisfies("v24.0.0", "^22.19.0 || >=24.0.0"));
    }

    #[test]
    fn missing_version_string_fails_closed() {
        assert!(!engine_satisfies("", "^22.19.0"));
        assert!(!engine_satisfies("not-a-version", "^22.19.0"));
    }

    #[test]
    fn tolerates_whitespace_around_or() {
        assert!(engine_satisfies(
            "v22.19.0",
            "^22.19.0   ||   >=24.0.0"
        ));
    }
}
