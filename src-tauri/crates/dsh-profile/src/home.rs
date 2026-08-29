//! `$DSH_HOME` resolution, mirroring upstream `home-paths`: the env var
//! (blank = unset) wins; otherwise `~/.dsh`.

use std::path::{Path, PathBuf};

pub const DSH_HOME_ENV: &str = "DSH_HOME";
pub const DSH_HOME_DIR_NAME: &str = ".dsh";
/// The only profile this client drives (`dsh web` ≡ `--profile web`).
pub const PROFILE_NAME: &str = "web";
pub const PROFILE_PATCH_FILENAME: &str = "cordis.patch.yml";

/// Pure core of [`resolve_dsh_home`]: the env getter is injected so tests
/// never touch the process environment (parallel-test safe).
pub fn dsh_home_from(get: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    if let Some(value) = get(DSH_HOME_ENV) {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }
    platform_home_from(get).map(|home| home.join(DSH_HOME_DIR_NAME))
}

#[cfg(windows)]
fn platform_home_from(get: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    get("USERPROFILE").map(PathBuf::from)
}

#[cfg(unix)]
fn platform_home_from(get: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    get("HOME").map(PathBuf::from)
}

/// Resolve against the real process environment. The daemon is our child and
/// inherits the same environment, so both sides resolve the same home.
pub fn resolve_dsh_home() -> Option<PathBuf> {
    dsh_home_from(|key| std::env::var(key).ok())
}

pub fn profile_dir(home: &Path) -> PathBuf {
    home.join("profiles").join(PROFILE_NAME)
}

pub fn patch_file(profile_dir: &Path) -> PathBuf {
    profile_dir.join(PROFILE_PATCH_FILENAME)
}

#[cfg(test)]
mod tests {
    use super::dsh_home_from;
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn getter(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |key| map.get(key).cloned()
    }

    #[cfg(windows)]
    const HOME_VAR: &str = "USERPROFILE";
    #[cfg(unix)]
    const HOME_VAR: &str = "HOME";

    #[test]
    fn env_override_wins() {
        let home = dsh_home_from(getter(&[
            ("DSH_HOME", "D:/dsh-data"),
            (HOME_VAR, "C:/Users/x"),
        ]))
        .unwrap();
        assert_eq!(home, PathBuf::from("D:/dsh-data"));
    }

    #[test]
    fn blank_env_falls_back_to_default() {
        for blank in ["", "   "] {
            let home =
                dsh_home_from(getter(&[("DSH_HOME", blank), (HOME_VAR, "/home/x")])).unwrap();
            assert_eq!(
                home,
                PathBuf::from("/home/x").join(".dsh"),
                "空白值 {blank:?} 应视为未设置"
            );
        }
    }

    #[test]
    fn missing_env_uses_home_dot_dsh() {
        let home = dsh_home_from(getter(&[(HOME_VAR, "/home/x")])).unwrap();
        assert_eq!(home, PathBuf::from("/home/x").join(".dsh"));
    }

    #[test]
    fn nothing_resolvable_yields_none() {
        assert!(dsh_home_from(getter(&[])).is_none());
    }

    #[test]
    fn profile_and_patch_paths_join() {
        let home = PathBuf::from("/data/.dsh");
        let profile = super::profile_dir(&home);
        assert_eq!(profile, PathBuf::from("/data/.dsh/profiles/web"));
        assert_eq!(
            super::patch_file(&profile),
            PathBuf::from("/data/.dsh/profiles/web/cordis.patch.yml")
        );
    }
}
