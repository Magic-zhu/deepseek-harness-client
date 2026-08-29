//! Launch-plan resolution: `DSH_CLIENT_BIN` override, then `dsh` on PATH,
//! then `npx -y @deepseek-ai/dsh@latest`. [`DshInvocation`] is the resolved
//! "how to reach the dsh CLI" (program plus binary-selecting prefix);
//! [`LaunchPlan`] is one concrete argv built from it. Spawning is the only
//! side effect, and it lives in [`LaunchPlan::spawn_env`].

use std::path::PathBuf;
use std::process::Stdio;

use tokio::process::{Child, Command};

/// Windows `CREATE_NO_WINDOW`: never flash a console for the daemon tree.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// How to launch `dsh web`: program plus the full argument vector.
#[derive(Debug, Clone)]
pub struct LaunchPlan {
    pub program: String,
    pub args: Vec<String>,
}

/// How to reach the dsh CLI on this machine: program plus the argument
/// prefix that selects the dsh binary (override tokens / npx package spec).
/// The subcommand and its args are appended by the caller.
#[derive(Debug, Clone)]
pub struct DshInvocation {
    pub program: String,
    pub prefix: Vec<String>,
}

impl DshInvocation {
    /// Build one concrete argv, e.g. `plan(&["web", "--port", "0"])`.
    pub fn plan(&self, args: &[&str]) -> LaunchPlan {
        let mut full = self.prefix.clone();
        full.extend(args.iter().map(|s| (*s).to_string()));
        LaunchPlan {
            program: self.program.clone(),
            args: full,
        }
    }

    /// One-line display for diagnostics; never executed.
    pub fn display(&self) -> String {
        format!("{} {}", self.program, self.prefix.join(" "))
    }
}

impl LaunchPlan {
    /// One-line display for diagnostics; never executed.
    pub fn display(&self) -> String {
        format!("{} {}", self.program, self.args.join(" "))
    }

    /// Spawn with piped stdout/stderr and a killed-with-us lifetime.
    pub async fn spawn(&self) -> std::io::Result<Child> {
        self.spawn_env(&[]).await
    }

    /// Spawn with extra environment (`CI=true` keeps pnpm non-interactive).
    pub async fn spawn_env(&self, extra_env: &[(&str, &str)]) -> std::io::Result<Child> {
        // npm shims on Windows are .cmd files: only cmd.exe can run them.
        #[cfg(windows)]
        let mut cmd = {
            let mut c = Command::new("cmd");
            c.arg("/c").arg(&self.program);
            c
        };
        #[cfg(unix)]
        let mut cmd = Command::new(&self.program);

        cmd.args(&self.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in extra_env {
            cmd.env(key, value);
        }
        // Dedicated npm cache: the machine-wide `_npx` cache is shared with
        // every other npm user on the host, and concurrent installs racing
        // on it abort with EPERM on Windows. The daemon gets its own.
        if std::env::var_os("npm_config_cache").is_none() {
            if let Ok(local) = std::env::var("LOCALAPPDATA") {
                let cache = std::path::Path::new(&local)
                    .join("dsh-client")
                    .join("npm-cache");
                cmd.env("npm_config_cache", &cache);
            }
        }
        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);
        #[cfg(unix)]
        cmd.process_group(0);
        cmd.spawn()
    }
}

/// Resolve how this machine reaches the dsh CLI.
pub fn resolve_invocation(bin_override: Option<&str>) -> DshInvocation {
    if let Some(bin) = bin_override.map(str::trim).filter(|s| !s.is_empty()) {
        let mut parts = bin.split_whitespace();
        let program = parts.next().unwrap_or("dsh").to_string();
        let prefix: Vec<String> = parts.map(String::from).collect();
        return DshInvocation { program, prefix };
    }
    if find_in_path("dsh").is_some() {
        return DshInvocation {
            program: "dsh".into(),
            prefix: Vec::new(),
        };
    }
    if find_in_path("npx").is_some() {
        return DshInvocation {
            program: "npx".into(),
            prefix: ["-y", "@deepseek-ai/dsh@latest"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
        };
    }
    // Nothing on PATH: keep the plain name so the spawn error names the target.
    DshInvocation {
        program: "dsh".into(),
        prefix: Vec::new(),
    }
}

/// Resolve how this machine launches `dsh web`.
pub fn resolve_launch(bin_override: Option<&str>) -> LaunchPlan {
    resolve_invocation(bin_override).plan(&["web", "--port", "0"])
}

pub fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    #[cfg(windows)]
    let candidates: Vec<String> = ["", ".cmd", ".exe", ".bat"]
        .iter()
        .flat_map(|ext| [ext.to_lowercase(), ext.to_uppercase()])
        .map(|ext| format!("{name}{ext}"))
        .collect();
    #[cfg(unix)]
    let candidates: Vec<String> = vec![name.to_string()];
    for dir in std::env::split_paths(&path) {
        for candidate in &candidates {
            let full = dir.join(candidate);
            if full.is_file() {
                return Some(full);
            }
        }
    }
    None
}

/// First whitespace-separated token of an override string. Mirrors the
/// parsing in [`resolve_launch`] so the preflight probe targets the same
/// binary that the supervisor would spawn.
pub fn extract_bin_token(bin_override: Option<&str>) -> Option<String> {
    bin_override
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| s.split_whitespace().next().map(str::to_owned))
}

/// First `node` on PATH; surfaced by the preflight so the UI can warn about
/// nvm shims pointing at the wrong version.
pub fn find_first_node_in_path() -> Option<PathBuf> {
    find_in_path("node")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_splits_program_and_prefix() {
        let invocation = resolve_invocation(Some("node C:\\nvm\\v24.15.0\\bin.js"));
        assert_eq!(invocation.program, "node");
        assert_eq!(invocation.prefix, vec!["C:\\nvm\\v24.15.0\\bin.js"]);
    }

    #[test]
    fn blank_override_falls_through_without_panic() {
        // PATH 分支依赖运行机器，只断言程序名非空。
        assert!(!resolve_invocation(Some("   ")).program.is_empty());
    }

    #[test]
    fn plan_appends_subcommand_args() {
        let invocation = resolve_invocation(Some("node /opt/dsh/bin.js"));
        let plan = invocation.plan(&["plugin", "--profile", "web", "add", "foo"]);
        assert_eq!(plan.program, "node");
        assert_eq!(
            plan.args,
            vec![
                "/opt/dsh/bin.js",
                "plugin",
                "--profile",
                "web",
                "add",
                "foo"
            ]
        );
    }

    #[test]
    fn resolve_launch_keeps_legacy_shape() {
        let plan = resolve_launch(Some("node /opt/dsh/bin.js"));
        assert_eq!(plan.args, vec!["/opt/dsh/bin.js", "web", "--port", "0"]);
    }
}
