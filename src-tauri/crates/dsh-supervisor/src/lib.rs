//! dsh-supervisor — process supervision for a `dsh web` sidecar.
//!
//! One job, mechanism only: keep a `dsh web` daemon available on this
//! machine, or say why it cannot be. Readiness is the daemon's own URL line
//! (`dsh web: http://127.0.0.1:<port>`): upstream guarantees the /api routes
//! are mounted when it prints, so observing the line is the whole handshake.
//! Crash recovery is exponential backoff; control is stop / restart.

mod preflight;
mod resolve;
#[cfg(windows)]
mod winjob;

use std::collections::VecDeque;
use std::process::ExitStatus;
use std::time::Duration;

use serde::Serialize;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::Child;
use tokio::sync::{broadcast, watch, Mutex, oneshot};

pub use preflight::{run_probe, PreflightReport, VersionSource, REQUIRED_ENGINE};
pub use resolve::{
    extract_bin_token, find_first_node_in_path, resolve_invocation, resolve_launch, DshInvocation,
    LaunchPlan,
};

/// Upstream's readiness contract printed on stdout.
const READY_PREFIX: &str = "dsh web: http://127.0.0.1:";
/// Retained log lines surfaced through [`Supervisor::log_tail`].
const LOG_TAIL_CAP: usize = 500;
/// Grace period after a kill before escalating to a hard terminate.
const KILL_GRACE: Duration = Duration::from_secs(5);
/// How long one attempt may take from spawn to the URL line. Generous: a
/// cold `npx` download of the daemon legitimately takes minutes.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(300);
/// Post-ready liveness probe cadence: supervise the *service*, not just
/// the process — an npm wrapper can hang long after its child died.
const PROBE_INTERVAL: Duration = Duration::from_secs(2);
const PROBE_TIMEOUT: Duration = Duration::from_secs(1);
const PROBE_FAILURES: u32 = 3;

/// Exponential backoff between launch attempts.
#[derive(Debug, Clone, Copy)]
pub struct RestartPolicy {
    pub initial: Duration,
    pub multiplier: f64,
    pub max: Duration,
}

impl Default for RestartPolicy {
    fn default() -> Self {
        Self {
            initial: Duration::from_millis(500),
            multiplier: 2.0,
            max: Duration::from_secs(30),
        }
    }
}

impl RestartPolicy {
    /// Delay before attempt N (counting from 1), capped at `max`.
    fn delay_for(&self, attempt: u32) -> Duration {
        let exp = self.multiplier.powi(attempt.saturating_sub(1) as i32);
        let ms = self.initial.as_millis() as f64 * exp;
        Duration::from_millis(ms.min(self.max.as_millis() as f64) as u64)
    }
}

/// Which child stream a log line came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LogStream {
    Stdout,
    Stderr,
}

/// Everything the outside world can observe about the supervised daemon.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum SupervisorEvent {
    /// A launch attempt began (attempt counts from 1).
    Starting { attempt: u32 },
    /// The daemon printed its URL line: /api is mounted and accepting.
    Ready { port: u16, pid: u32 },
    /// The child died or never became ready; next attempt after `retry_in_ms`.
    Crashed { attempt: u32, reason: String, retry_in_ms: u64 },
    /// Supervision ended by request; the child tree is gone.
    Stopped,
    /// One stdout/stderr line from the child tree.
    Log { stream: LogStream, line: String },
}

/// Coarse lifecycle state for status displays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Starting,
    Running,
    Backoff,
    Stopped,
}

/// Snapshot of the supervisor's current view.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub state: State,
    pub pid: Option<u32>,
    pub port: Option<u16>,
    pub attempt: u32,
    pub restarts: u32,
    pub last_error: Option<String>,
}

/// Control-plane commands the owner can send at any time.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Control {
    Run,
    Stop,
    Restart,
}

/// The tree-kill capability registered per attempt.
enum KillHandle {
    /// Held only for its Drop: closing the job takes the tree down.
    #[cfg(windows)]
    Job { _guard: winjob::JobGuard },
    #[cfg(unix)]
    Group { pid: i32 },
}

struct Inner {
    launch: LaunchPlan,
    policy: RestartPolicy,
    events: broadcast::Sender<SupervisorEvent>,
    control: watch::Sender<Control>,
    status: Mutex<Status>,
    log_tail: Mutex<VecDeque<(LogStream, String)>>,
    kill: Mutex<Option<KillHandle>>,
}

impl Inner {
    async fn begin(&self, attempt: u32) {
        let mut status = self.status.lock().await;
        status.state = State::Starting;
        status.attempt = attempt;
        status.pid = None;
        status.port = None;
        status.last_error = None;
    }

    async fn ready(&self, port: u16, pid: u32) {
        let mut status = self.status.lock().await;
        status.state = State::Running;
        status.port = Some(port);
        status.pid = Some(pid);
    }

    async fn backoff(&self, reason: String, delay: Duration) {
        let mut status = self.status.lock().await;
        status.state = State::Backoff;
        status.pid = None;
        status.port = None;
        status.restarts += 1;
        status.last_error = Some(format!("{reason} (retry in {delay:?})"));
    }

    async fn stopped(&self) {
        let mut status = self.status.lock().await;
        status.state = State::Stopped;
        status.pid = None;
        status.port = None;
    }
}

/// Handle to a running supervision loop. Cheap to clone; `stop`/`restart`
/// are the only writes, status reads are snapshots.
#[derive(Clone)]
pub struct Supervisor {
    inner: std::sync::Arc<Inner>,
}

impl Supervisor {
    /// Start supervising. Must be called inside a tokio runtime context.
    pub fn start(launch: LaunchPlan, policy: RestartPolicy) -> Self {
        let (events, _) = broadcast::channel(512);
        let (control, control_rx) = watch::channel(Control::Run);
        let inner = std::sync::Arc::new(Inner {
            launch,
            policy,
            events,
            control,
            status: Mutex::new(Status {
                state: State::Starting,
                pid: None,
                port: None,
                attempt: 1,
                restarts: 0,
                last_error: None,
            }),
            log_tail: Mutex::new(VecDeque::new()),
            kill: Mutex::new(None),
        });
        tokio::spawn(run_loop(inner.clone(), control_rx));
        Supervisor { inner }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<SupervisorEvent> {
        self.inner.events.subscribe()
    }

    pub fn command_display(&self) -> String {
        self.inner.launch.display()
    }

    /// Kill the current tree and relaunch immediately (resets the backoff).
    pub fn restart(&self) {
        let _ = self.inner.control.send(Control::Restart);
    }

    /// End supervision and take the child tree down.
    pub fn stop(&self) {
        let _ = self.inner.control.send(Control::Stop);
    }

    pub async fn status(&self) -> Status {
        self.inner.status.lock().await.clone()
    }

    pub async fn log_tail(&self, max: usize) -> Vec<(LogStream, String)> {
        let tail = self.inner.log_tail.lock().await;
        let skip = tail.len().saturating_sub(max);
        tail.iter().skip(skip).cloned().collect()
    }
}

/// How the run phase of one attempt ended.
enum RunOutcome {
    /// The child exited on its own (Ok) or wait() failed.
    Exited(std::io::Result<ExitStatus>),
    /// A stop/restart command arrived while the child lived.
    Control(Control),
    /// Ready fired, then the port stopped answering while the process tree
    /// stayed alive (hung wrapper). Killed by the loop before returning.
    ServiceDead,
}

/// How a crash/backoff phase ended.
enum Flow {
    /// Backoff elapsed: relaunch normally.
    Continue,
    /// A restart arrived during backoff: relaunch now with a reset counter.
    Restart,
    /// A stop arrived: end the loop.
    Stop,
}

async fn run_loop(inner: std::sync::Arc<Inner>, mut control: watch::Receiver<Control>) {
    let mut attempt: u32 = 0;
    'supervise: loop {
        attempt += 1;
        inner.begin(attempt).await;
        let _ = inner.events.send(SupervisorEvent::Starting { attempt });

        let mut child = match inner.launch.spawn().await {
            Ok(child) => child,
            Err(err) => {
                match backoff_or_stop(&inner, attempt, format!("spawn failed: {err}"), &mut control)
                    .await
                {
                    Flow::Continue => continue 'supervise,
                    Flow::Restart => {
                        attempt = 0;
                        continue 'supervise;
                    }
                    Flow::Stop => {
                        finish_stop(&inner).await;
                        return;
                    }
                }
            }
        };
        let pid = child.id().unwrap_or_default();
        register_kill(&inner, pid).await;

        let (ready_tx, mut ready_rx) = oneshot::channel::<u16>();
        if let Some(stdout) = child.stdout.take() {
            spawn_reader(inner.clone(), stdout, LogStream::Stdout, Some(ready_tx));
        }
        if let Some(stderr) = child.stderr.take() {
            spawn_reader(inner.clone(), stderr, LogStream::Stderr, None);
        }

        // Phase A: from spawn to the URL line (or early exit/control).
        let port = tokio::select! {
            res = child.wait() => {
                let reason = match res {
                    Ok(status) => describe_exit(&status),
                    Err(err) => format!("wait failed: {err}"),
                };
                match backoff_or_stop(&inner, attempt, reason, &mut control).await {
                    Flow::Continue => continue 'supervise,
                    Flow::Restart => { attempt = 0; continue 'supervise; }
                    Flow::Stop => { finish_stop(&inner).await; return; }
                }
            }
            Ok(port) = &mut ready_rx => port,
            _ = control.changed() => {
                let other = *control.borrow_and_update();
                kill_and_reap(&inner, &mut child, pid).await;
                match other {
                    Control::Stop => {
                        finish_stop(&inner).await;
                        return;
                    }
                    Control::Restart => {
                        attempt = 0;
                        continue 'supervise;
                    }
                    Control::Run => unreachable!("Run is the initial value, never re-sent"),
                }
            }
            _ = tokio::time::sleep(STARTUP_TIMEOUT) => {
                kill_and_reap(&inner, &mut child, pid).await;
                let reason = format!("no ready line within {STARTUP_TIMEOUT:?}");
                match backoff_or_stop(&inner, attempt, reason, &mut control).await {
                    Flow::Continue => continue 'supervise,
                    Flow::Restart => { attempt = 0; continue 'supervise; }
                    Flow::Stop => { finish_stop(&inner).await; return; }
                }
            }
        };

        inner.ready(port, pid).await;
        let _ = inner.events.send(SupervisorEvent::Ready { port, pid });

        // Phase B: the service is up — keep watching the process *and* the
        // port. A hung wrapper with a dead server must count as a crash.
        let (dead_tx, dead_rx) = oneshot::channel::<()>();
        let probe = tokio::spawn(probe_loop(port, dead_tx));
        let outcome = tokio::select! {
            res = child.wait() => RunOutcome::Exited(res),
            _ = control.changed() => RunOutcome::Control(*control.borrow_and_update()),
            _ = dead_rx => RunOutcome::ServiceDead,
        };
        probe.abort();

        match outcome {
            RunOutcome::Control(Control::Stop) => {
                kill_and_reap(&inner, &mut child, pid).await;
                finish_stop(&inner).await;
                return;
            }
            RunOutcome::Control(Control::Restart) => {
                kill_and_reap(&inner, &mut child, pid).await;
                attempt = 0;
                continue 'supervise;
            }
            RunOutcome::Control(Control::Run) => unreachable!("never re-sent"),
            RunOutcome::Exited(Ok(status)) => {
                let reason = describe_exit(&status);
                match backoff_or_stop(&inner, attempt, reason, &mut control).await {
                    Flow::Continue => continue 'supervise,
                    Flow::Restart => { attempt = 0; continue 'supervise; }
                    Flow::Stop => { finish_stop(&inner).await; return; }
                }
            }
            RunOutcome::Exited(Err(err)) => {
                let reason = format!("wait failed: {err}");
                match backoff_or_stop(&inner, attempt, reason, &mut control).await {
                    Flow::Continue => continue 'supervise,
                    Flow::Restart => { attempt = 0; continue 'supervise; }
                    Flow::Stop => { finish_stop(&inner).await; return; }
                }
            }
            RunOutcome::ServiceDead => {
                kill_and_reap(&inner, &mut child, pid).await;
                let reason = "service stopped responding on its port".to_string();
                match backoff_or_stop(&inner, attempt, reason, &mut control).await {
                    Flow::Continue => continue 'supervise,
                    Flow::Restart => { attempt = 0; continue 'supervise; }
                    Flow::Stop => { finish_stop(&inner).await; return; }
                }
            }
        }
    }
}

/// Enter the stopped state and announce it on every exit path alike.
async fn finish_stop(inner: &std::sync::Arc<Inner>) {
    inner.stopped().await;
    let _ = inner.events.send(SupervisorEvent::Stopped);
}

async fn backoff_or_stop(
    inner: &std::sync::Arc<Inner>,
    attempt: u32,
    reason: String,
    control: &mut watch::Receiver<Control>,
) -> Flow {
    let delay = inner.policy.delay_for(attempt);
    inner.backoff(reason.clone(), delay).await;
    let _ = inner.events.send(SupervisorEvent::Crashed {
        attempt,
        reason,
        retry_in_ms: delay.as_millis() as u64,
    });
    tokio::select! {
        _ = tokio::time::sleep(delay) => Flow::Continue,
        _ = control.changed() => match *control.borrow_and_update() {
            Control::Run => Flow::Continue,
            Control::Restart => Flow::Restart,
            Control::Stop => Flow::Stop,
        },
    }
}

async fn register_kill(inner: &std::sync::Arc<Inner>, pid: u32) {
    #[cfg(windows)]
    {
        let handle = winjob::assign_pid(pid)
            .ok()
            .map(|guard| KillHandle::Job { _guard: guard });
        *inner.kill.lock().await = handle;
    }
    #[cfg(unix)]
    {
        let _ = pid;
        // process_group(0) at spawn made the pgid equal the pid.
        *inner.kill.lock().await = Some(KillHandle::Group { pid: pid as i32 });
    }
}

async fn kill_and_reap(inner: &std::sync::Arc<Inner>, child: &mut Child, pid: u32) {
    // The graceful signal is taking the kill handle: on Unix that is SIGTERM
    // to the process group; on Windows closing the job hard-terminates the
    // tree (dsh's append-only session log makes that safe).
    #[cfg(windows)]
    {
        *inner.kill.lock().await = None;
    }
    #[cfg(unix)]
    {
        if let Some(KillHandle::Group { pid }) = inner.kill.lock().await.take() {
            unsafe {
                libc::killpg(pid, libc::SIGTERM);
            }
        }
    }
    if tokio::time::timeout(KILL_GRACE, child.wait()).await.is_err() {
        #[cfg(unix)]
        unsafe {
            libc::killpg(pid as i32, libc::SIGKILL);
        }
        #[cfg(windows)]
        winjob::terminate_pid(pid);
        let _ = child.wait().await;
    }
}

/// Announce on `dead` once the daemon port stops accepting connections for
/// [`PROBE_FAILURES`] consecutive probes.
async fn probe_loop(port: u16, dead: oneshot::Sender<()>) {
    let mut failures = 0u32;
    loop {
        tokio::time::sleep(PROBE_INTERVAL).await;
        if port_accepts(port).await {
            failures = 0;
        } else {
            failures += 1;
            if failures >= PROBE_FAILURES {
                let _ = dead.send(());
                return;
            }
        }
    }
}

async fn port_accepts(port: u16) -> bool {
    tokio::time::timeout(
        PROBE_TIMEOUT,
        tokio::net::TcpStream::connect(("127.0.0.1", port)),
    )
    .await
    .map(|result| result.is_ok())
    .unwrap_or(false)
}

fn spawn_reader(
    inner: std::sync::Arc<Inner>,
    source: impl AsyncRead + Unpin + Send + 'static,
    stream: LogStream,
    ready: Option<oneshot::Sender<u16>>,
) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(source).lines();
        let mut ready = ready;
        while let Ok(Some(line)) = lines.next_line().await {
            if stream == LogStream::Stdout {
                if let (Some(tx), Some(port)) = (ready.take(), parse_ready_port(&line)) {
                    let _ = tx.send(port);
                }
            }
            push_log(&inner, stream, line).await;
        }
    });
}

async fn push_log(inner: &std::sync::Arc<Inner>, stream: LogStream, line: String) {
    {
        let mut tail = inner.log_tail.lock().await;
        tail.push_back((stream, line.clone()));
        while tail.len() > LOG_TAIL_CAP {
            tail.pop_front();
        }
    }
    let _ = inner
        .events
        .send(SupervisorEvent::Log { stream, line });
}

/// Extract the port from `dsh web: http://127.0.0.1:<port>` (anywhere in the
/// line; upstream may append a LAN variant).
fn parse_ready_port(line: &str) -> Option<u16> {
    let rest = &line[line.find(READY_PREFIX)? + READY_PREFIX.len()..];
    rest.split(|c: char| !c.is_ascii_digit())
        .next()
        .filter(|digits| !digits.is_empty())
        .and_then(|digits| digits.parse().ok())
}

fn describe_exit(status: &ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("exit code {code}"),
        None => "killed by signal".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_ready_line() {
        assert_eq!(
            parse_ready_port("dsh web: http://127.0.0.1:39187"),
            Some(39187)
        );
        assert_eq!(
            parse_ready_port("dsh web: http://127.0.0.1:39187 (LAN: http://192.168.1.4:39187)"),
            Some(39187)
        );
        assert_eq!(parse_ready_port("some unrelated line"), None);
        assert_eq!(parse_ready_port("dsh web: http://127.0.0.1:"), None);
    }

    #[test]
    fn backoff_grows_and_caps() {
        let policy = RestartPolicy::default();
        assert_eq!(policy.delay_for(1), Duration::from_millis(500));
        assert_eq!(policy.delay_for(2), Duration::from_millis(1000));
        assert_eq!(policy.delay_for(3), Duration::from_millis(2000));
        assert_eq!(policy.delay_for(20), policy.max);
    }
}
