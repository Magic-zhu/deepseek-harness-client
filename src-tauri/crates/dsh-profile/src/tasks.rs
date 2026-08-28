//! Serial runner for `dsh plugin --profile web add|remove <spec>`.
//!
//! pnpm holds a lock on the profile directory, so tasks run strictly one at
//! a time; output streams into a bounded tail and every state change is
//! broadcast for the UI. Reuses the supervisor's launch resolution so the
//! CLI binary is the same one the daemon came from.

use std::sync::{Arc, Mutex};

use dsh_supervisor::DshInvocation;
use serde::Serialize;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{broadcast, mpsc};

/// Output ring capacity, aligned with dsh-supervisor's log tail.
pub const OUTPUT_TAIL_CAP: usize = 500;
/// Finished tasks retained for a late-joining window.
const FINISHED_RETAIN: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PluginTaskKind {
    Install,
    Remove,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PluginTaskStatus {
    Running,
    Done,
    Failed,
}

/// `plugins://task` event payload (serde camelCase on the wire).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginTaskView {
    pub task_id: String,
    pub kind: PluginTaskKind,
    pub spec: String,
    pub status: PluginTaskStatus,
    pub output_tail: Vec<String>,
    pub exit_code: Option<i32>,
}

struct Job {
    task_id: String,
    kind: PluginTaskKind,
    spec: String,
}

/// A spec is one argv token; on Windows it crosses `cmd /c`, which
/// re-parses the line — metacharacters are refused outright.
fn validate_spec(spec: &str) -> Result<(), String> {
    if spec.is_empty() || spec.trim() != spec {
        return Err("spec 为空或含首尾空白".to_string());
    }
    if spec.chars().any(|c| c.is_whitespace() || "&|<>^\"\r\n".contains(c)) {
        return Err(format!("spec 含非法字符：{spec:?}"));
    }
    Ok(())
}

fn push_line(view: &mut PluginTaskView, line: String) {
    view.output_tail.push(line);
    if view.output_tail.len() > OUTPUT_TAIL_CAP {
        let overflow = view.output_tail.len() - OUTPUT_TAIL_CAP;
        view.output_tail.drain(..overflow);
    }
}

#[derive(Clone)]
pub struct PluginTaskRunner {
    sender: mpsc::UnboundedSender<Job>,
    views: Arc<Mutex<Vec<PluginTaskView>>>,
    events: broadcast::Sender<PluginTaskView>,
}

impl PluginTaskRunner {
    /// Spawn the serial worker on the current tokio runtime.
    pub fn start(invocation: DshInvocation) -> Self {
        let (sender, receiver) = mpsc::unbounded_channel::<Job>();
        let (events, _) = broadcast::channel(64);
        let runner = Self { sender, views: Arc::new(Mutex::new(Vec::new())), events };
        let worker = runner.clone();
        tokio::spawn(async move { worker_loop(invocation, receiver, worker).await });
        runner
    }

    /// Queue one task; the task id returns immediately, progress comes via
    /// [`PluginTaskRunner::subscribe`].
    pub fn submit(&self, kind: PluginTaskKind, spec: String) -> Result<String, String> {
        validate_spec(&spec)?;
        let task_id = uuid::Uuid::new_v4().to_string();
        let view = PluginTaskView {
            task_id: task_id.clone(),
            kind,
            spec: spec.clone(),
            status: PluginTaskStatus::Running,
            output_tail: Vec::new(),
            exit_code: None,
        };
        self.views.lock().expect("views lock poisoned").push(view.clone());
        let _ = self.events.send(view);
        self.sender
            .send(Job { task_id: task_id.clone(), kind, spec })
            .map_err(|_| "任务队列已关闭".to_string())?;
        Ok(task_id)
    }

    pub fn list(&self) -> Vec<PluginTaskView> {
        self.views.lock().expect("views lock poisoned").clone()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<PluginTaskView> {
        self.events.subscribe()
    }

    fn with_view(&self, task_id: &str, f: impl FnOnce(&mut PluginTaskView)) {
        let view = {
            let mut views = self.views.lock().expect("views lock poisoned");
            let Some(view) = views.iter_mut().find(|v| v.task_id == task_id) else { return };
            f(view);
            view.clone()
        };
        let _ = self.events.send(view);
    }

    fn finish(&self, task_id: &str, status: PluginTaskStatus, exit_code: Option<i32>, line: String) {
        self.with_view(task_id, |view| {
            push_line(view, line);
            view.status = status;
            view.exit_code = exit_code;
        });
        // Retention: bound finished tasks, oldest first.
        let mut views = self.views.lock().expect("views lock poisoned");
        let finished = views.iter().filter(|v| v.status != PluginTaskStatus::Running).count();
        let mut remove = finished.saturating_sub(FINISHED_RETAIN);
        if remove > 0 {
            views.retain(|v| {
                if remove > 0 && v.status != PluginTaskStatus::Running {
                    remove -= 1;
                    false
                } else {
                    true
                }
            });
        }
    }
}

/// Stream one piped child output into the task's tail, line by line.
fn spawn_reader<S>(stream: S, runner: PluginTaskRunner, task_id: String) -> tokio::task::JoinHandle<()>
where
    S: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(stream).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            runner.with_view(&task_id, |view| push_line(view, line));
        }
    })
}

async fn worker_loop(
    invocation: DshInvocation,
    mut receiver: mpsc::UnboundedReceiver<Job>,
    runner: PluginTaskRunner,
) {
    while let Some(job) = receiver.recv().await {
        run_one(&invocation, &job, &runner).await;
    }
}

async fn run_one(invocation: &DshInvocation, job: &Job, runner: &PluginTaskRunner) {
    let verb = match job.kind {
        PluginTaskKind::Install => "add",
        PluginTaskKind::Remove => "remove",
    };
    let plan = invocation.plan(&["plugin", "--profile", "web", verb, &job.spec]);
    runner.with_view(&job.task_id, |view| push_line(view, format!("$ {}", plan.display())));

    let spawned = plan.spawn_env(&[("CI", "true")]).await;
    let mut child = match spawned {
        Ok(child) => child,
        Err(err) => {
            runner.finish(&job.task_id, PluginTaskStatus::Failed, None, format!("启动失败：{err}"));
            return;
        }
    };

    // stdout/stderr 类型不同（ChildStdout/ChildStderr），用泛型 reader。
    let stdout = child.stdout.take().expect("spawn pipes stdout");
    let stderr = child.stderr.take().expect("spawn pipes stderr");
    let h1 = spawn_reader(stdout, runner.clone(), job.task_id.clone());
    let h2 = spawn_reader(stderr, runner.clone(), job.task_id.clone());
    let _ = tokio::join!(h1, h2);

    match child.wait().await {
        Ok(status) if status.success() => {
            runner.finish(&job.task_id, PluginTaskStatus::Done, status.code(), "完成".into());
        }
        Ok(status) => {
            runner.finish(&job.task_id, PluginTaskStatus::Failed, status.code(), format!("退出码 {:?}", status.code()));
        }
        Err(err) => {
            runner.finish(&job.task_id, PluginTaskStatus::Failed, None, format!("等待退出失败：{err}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_spec_rejects_shell_metacharacters() {
        // Windows 下经 cmd /c 透传，& | < > ^ " 与空白都可能被 cmd 重新解释。
        for bad in ["", " a", "a ", "a b", "a&calc", "a|b", "a>b", "a\"b", "a^b", "a\nb"] {
            assert!(validate_spec(bad).is_err(), "应拒绝 {bad:?}");
        }
        // 注：caret-range（@scope/name@^1.2.3）经 cmd /c 时 ^ 被吞、语义被
        // 静默改成精确版本，故与 a^b 一样被拒；合法用例用精确版本号验证。
        assert!(validate_spec("@scope/name@1.2.3").is_ok());
        assert!(validate_spec("github:user/repo").is_ok());
    }

    #[test]
    fn output_tail_is_capped() {
        let mut view = PluginTaskView {
            task_id: "t".into(),
            kind: PluginTaskKind::Install,
            spec: "s".into(),
            status: PluginTaskStatus::Running,
            output_tail: Vec::new(),
            exit_code: None,
        };
        for i in 0..600 {
            push_line(&mut view, format!("line-{i}"));
        }
        assert_eq!(view.output_tail.len(), OUTPUT_TAIL_CAP);
        assert_eq!(view.output_tail[0], "line-100");
    }

    /// 假 dsh：把每次调用的 argv 追加到日志，中间睡一拍。两个任务排队后，
    /// 日志顺序必须是 [start a, end a, start b, end b]——第二个任务不得穿插。
    #[tokio::test(flavor = "multi_thread")]
    async fn tasks_run_strictly_serial() {
        let dir = std::env::temp_dir().join(format!("dsh-runner-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("calls.log");

        #[cfg(windows)]
        let program = {
            let script = dir.join("fake-dsh.cmd");
            std::fs::write(
                &script,
                format!(
                    "@echo off\r\necho start %* >> \"{}\"\r\nping -n 2 127.0.0.1 >nul\r\necho end %* >> \"{}\"\r\n",
                    log.display(),
                    log.display()
                ),
            )
            .unwrap();
            script.to_string_lossy().into_owned()
        };
        #[cfg(unix)]
        let program = {
            use std::os::unix::fs::PermissionsExt;
            let script = dir.join("fake-dsh.sh");
            std::fs::write(
                &script,
                format!("#!/bin/sh\necho start \"$@\" >> \"{}\"\nsleep 1\necho end \"$@\" >> \"{}\"\n", log.display(), log.display()),
            )
            .unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
            script.to_string_lossy().into_owned()
        };

        let runner = PluginTaskRunner::start(DshInvocation { program, prefix: vec![] });
        let t1 = runner.submit(PluginTaskKind::Install, "spec-a".into()).unwrap();
        let t2 = runner.submit(PluginTaskKind::Remove, "spec-b".into()).unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            let views = runner.list();
            let done = |id: &str| views.iter().any(|v| v.task_id == id && v.status == PluginTaskStatus::Done);
            if done(&t1) && done(&t2) {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "任务未在 15s 内完成：{:?}", runner.list());
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }

        let calls = std::fs::read_to_string(&log).unwrap();
        let order: Vec<&str> = calls
            .lines()
            .map(|l| if l.contains("spec-a") { "a" } else { "b" })
            .collect();
        assert_eq!(order, ["a", "a", "b", "b"], "串行执行：{calls}");
    }
}
