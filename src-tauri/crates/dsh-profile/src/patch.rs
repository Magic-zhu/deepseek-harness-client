//! Text-level managed lines in the user patch layer (`cordis.patch.yml`).
//!
//! The file may contain `!!js` expressions, so it is never YAML-parsed or
//! evaluated. We only touch lines carrying our own end-of-line marker, and
//! every write is explicit intent: drop all our blocks for the entry, then
//! append exactly one.

use std::path::{Path, PathBuf};

/// End-of-line marker identifying every line this client owns.
pub const MARKER: &str = "# dsh-client";
/// First-write backup suffix, created once and never overwritten.
pub const BACKUP_SUFFIX: &str = ".dsh-client.bak";
/// Atomic-write scratch suffix, renamed over the target.
const TMP_SUFFIX: &str = ".dsh-client.tmp";

/// Patch write failure: message names the path and the cause.
#[derive(Debug)]
pub struct PatchError {
    pub path: PathBuf,
    pub message: String,
}

impl std::fmt::Display for PatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}：{}", self.path.display(), self.message)
    }
}

impl std::error::Error for PatchError {}

/// Reject entry ids that could break the two-line managed block shape or
/// smuggle YAML into the file.
pub fn validate_entry_id(entry_id: &str) -> Result<(), String> {
    if entry_id.is_empty() || entry_id.trim() != entry_id {
        return Err(format!("entryId 为空或含首尾空白：{entry_id:?}"));
    }
    if entry_id.contains(['\n', '\r', '#']) {
        return Err(format!("entryId 含非法字符：{entry_id:?}"));
    }
    Ok(())
}

fn is_marked(line: &str) -> bool {
    line.trim_end().ends_with(MARKER)
}

/// `- id: <entryId>  # dsh-client`，标记前的间距不敏感，id 精确匹配
/// （`a` 不匹配 `a:b`）。
fn is_managed_id_line(line: &str, entry_id: &str) -> bool {
    let Some(head) = line.trim_end().strip_suffix(MARKER) else {
        return false;
    };
    head.trim_end() == format!("- id: {entry_id}")
}

/// Apply the explicit intent "entry `entry_id` disabled = `disabled`" to
/// patch file text, returning the new text. Idempotent.
pub fn apply_set_disabled(text: &str, entry_id: &str, disabled: bool) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        if is_managed_id_line(line, entry_id) {
            // Drop the id line plus our marked continuation lines. Ours are
            // indented and never start a fresh `- ` item, so a foreign entry
            // cannot be eaten.
            while let Some(next) = lines.peek() {
                if is_marked(next) && !next.trim_start().starts_with("- ") {
                    lines.next();
                } else {
                    break;
                }
            }
            continue;
        }
        out.push(line);
    }
    let mut result = if out.is_empty() { String::new() } else { out.join("\n") + "\n" };
    result.push_str(&format!("- id: {entry_id}  {MARKER}\n  disabled: {disabled}  {MARKER}\n"));
    result
}

fn sibling_with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("cordis.patch.yml");
    path.with_file_name(format!("{name}{suffix}"))
}

/// Write the explicit intent to the patch file at `patch_path`.
///
/// - Missing parent directory → guidance error (profile not initialized).
/// - First write copies the file to `<file>.dsh-client.bak` (never overwritten).
/// - Atomic: scratch file + rename, mirroring upstream include's writeback.
/// - No-op when the intent is already realized (keeps mtime stable, so the
///   upstream HMR watcher does not see phantom writes).
pub fn set_disabled(patch_path: &Path, entry_id: &str, disabled: bool) -> Result<(), PatchError> {
    let err = |message: String| PatchError { path: patch_path.to_path_buf(), message };
    validate_entry_id(entry_id).map_err(&err)?;

    let parent = patch_path.parent().expect("patch file path has a parent");
    if !parent.is_dir() {
        return Err(err("profile 目录不存在；请先启动一次 daemon 以初始化 profile".into()));
    }

    let (original, existed) = match std::fs::read_to_string(patch_path) {
        Ok(text) => (text, true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (String::new(), false),
        Err(e) => return Err(err(format!("读取失败：{e}"))),
    };

    let next = apply_set_disabled(&original, entry_id, disabled);
    if next == original {
        return Ok(());
    }

    if existed {
        let backup = sibling_with_suffix(patch_path, BACKUP_SUFFIX);
        if !backup.exists() {
            std::fs::copy(patch_path, &backup).map_err(|e| err(format!("备份失败：{e}")))?;
        }
    }

    let tmp = sibling_with_suffix(patch_path, TMP_SUFFIX);
    std::fs::write(&tmp, next).map_err(|e| err(format!("写入临时文件失败：{e}")))?;
    std::fs::rename(&tmp, patch_path).map_err(|e| err(format!("原子替换失败：{e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{apply_set_disabled, set_disabled, validate_entry_id, BACKUP_SUFFIX};
    use std::path::PathBuf;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dsh-profile-test-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn apply_to_empty_text_writes_one_block() {
        let out = apply_set_disabled("", "assistant/memory", true);
        assert_eq!(out, "- id: assistant/memory  # dsh-client\n  disabled: true  # dsh-client\n");
    }

    #[test]
    fn apply_is_idempotent() {
        let once = apply_set_disabled("", "a", true);
        assert_eq!(apply_set_disabled(&once, "a", true), once);
    }

    #[test]
    fn enable_writes_explicit_disabled_false() {
        let out = apply_set_disabled("", "a", false);
        assert!(out.contains("disabled: false"), "enable 也写显式行：{out}");
    }

    #[test]
    fn preserves_foreign_lines_including_bang_bang_js() {
        let foreign = "- insert:\n    - id: x\n      name: '@scope/x'\n      disabled: !!js process.platform === 'win32'\n- id: y\n  config:\n    k: 1\n";
        let out = apply_set_disabled(foreign, "a", true);
        assert!(out.starts_with(foreign), "他行原样保留：{out}");
        assert!(out.contains("- id: a  # dsh-client"));
    }

    #[test]
    fn flips_existing_block_without_duplicating() {
        let first = apply_set_disabled("", "a", true);
        let flipped = apply_set_disabled(&first, "a", false);
        assert_eq!(flipped.matches("- id: a  # dsh-client").count(), 1, "同一 entry 恰好一条：{flipped}");
        assert!(flipped.contains("disabled: false"));
    }

    #[test]
    fn entry_a_does_not_match_nested_a_b() {
        let text = "- id: a:b  # dsh-client\n  disabled: true  # dsh-client\n";
        let out = apply_set_disabled(text, "a", false);
        assert!(out.contains("- id: a:b  # dsh-client"), "前缀不误伤嵌套 id：{out}");
    }

    #[test]
    fn drops_stale_disabled_continuation_of_same_entry() {
        // 手工改乱过的文件：同一 entry 两条我方块，应收敛为一条。
        let messy = "- id: a  # dsh-client\n  disabled: true  # dsh-client\n- id: b\n  disabled: true\n- id: a  # dsh-client\n  disabled: true  # dsh-client\n";
        let out = apply_set_disabled(messy, "a", false);
        assert_eq!(out.matches("- id: a  # dsh-client").count(), 1);
        assert!(out.contains("- id: b\n  disabled: true\n"), "非我方行不动：{out}");
    }

    #[test]
    fn validate_entry_id_rejects_injection_chars() {
        for bad in ["", " a", "a ", "a\nb", "a\rb", "a#b"] {
            assert!(validate_entry_id(bad).is_err(), "应拒绝 {bad:?}");
        }
        assert!(validate_entry_id("a:b/c-d_e").is_ok());
    }

    #[test]
    fn set_disabled_creates_file_backup_and_is_atomic() {
        let dir = temp_dir("write");
        let patch = dir.join("cordis.patch.yml");
        std::fs::write(&patch, "- insert:\n    - id: x\n").unwrap();

        set_disabled(&patch, "a", true).unwrap();
        let text = std::fs::read_to_string(&patch).unwrap();
        assert!(text.contains("- id: a  # dsh-client"));

        let backup = dir.join(format!("cordis.patch.yml{BACKUP_SUFFIX}"));
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), "- insert:\n    - id: x\n", "首写备份为原始内容");

        // 第二次写入不覆盖备份。
        std::fs::write(&backup, "手工改过的备份").unwrap();
        set_disabled(&patch, "a", false).unwrap();
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), "手工改过的备份");
    }

    #[test]
    fn set_disabled_creates_missing_file_without_backup() {
        let dir = temp_dir("create");
        let patch = dir.join("cordis.patch.yml");
        set_disabled(&patch, "a", true).unwrap();
        assert!(patch.is_file());
        assert!(!dir.join(format!("cordis.patch.yml{BACKUP_SUFFIX}")).exists(), "无原件则无备份");
    }

    #[test]
    fn set_disabled_guides_when_profile_dir_missing() {
        let dir = temp_dir("missing");
        let patch = dir.join("nope").join("cordis.patch.yml");
        let err = set_disabled(&patch, "a", true).unwrap_err();
        assert!(err.to_string().contains("先启动一次 daemon"), "{err}");
    }
}
