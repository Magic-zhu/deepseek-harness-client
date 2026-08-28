//! Windows end-to-end smoke tests: supervise fake `dsh web` daemons — real
//! node HTTP servers — through the actual spawn/readiness/probe/backoff/stop
//! loop. Fakes print the upstream URL line, so `Ready` proves the parser and
//! reader pipeline against a live child process.

#![cfg(windows)]

use std::time::Duration;

use dsh_supervisor::{LaunchPlan, RestartPolicy, Supervisor, SupervisorEvent};
use tokio::sync::broadcast::Receiver;

fn write_script(name: &str, body: &str) -> String {
    let path = std::env::temp_dir().join(name);
    std::fs::write(&path, body).expect("write fake daemon script");
    path.to_string_lossy().into_owned()
}

fn plan_for(script: &str) -> LaunchPlan {
    LaunchPlan {
        program: "node".into(),
        args: vec![script.into()],
    }
}

async fn next_event(receiver: &mut Receiver<SupervisorEvent>) -> SupervisorEvent {
    tokio::time::timeout(Duration::from_secs(25), receiver.recv())
        .await
        .expect("event within 25s")
        .expect("channel stays open")
}

/// Lifecycle events only: `Log` is an independent interleaved stream.
async fn next_lifecycle(receiver: &mut Receiver<SupervisorEvent>) -> SupervisorEvent {
    loop {
        match next_event(receiver).await {
            SupervisorEvent::Log { .. } => continue,
            event => return event,
        }
    }
}

#[tokio::test]
async fn observes_ready_line_then_stops_cleanly() {
    // Prints the upstream readiness line and serves until killed.
    let script = write_script(
        "dsh-supervisor-test-ready.js",
        r#"
const server = require('http').createServer((req, res) => res.end('ok'))
server.listen(39123, '127.0.0.1', () => {
  console.log('dsh web: http://127.0.0.1:39123')
})
"#,
    );
    let supervisor = Supervisor::start(plan_for(&script), RestartPolicy::default());
    let mut receiver = supervisor.subscribe();

    match next_lifecycle(&mut receiver).await {
        SupervisorEvent::Starting { attempt } => assert_eq!(attempt, 1),
        other => panic!("expected Starting, got {other:?}"),
    }
    match next_lifecycle(&mut receiver).await {
        SupervisorEvent::Ready { port, .. } => assert_eq!(port, 39123),
        other => panic!("expected Ready, got {other:?}"),
    }

    supervisor.stop();
    match next_lifecycle(&mut receiver).await {
        SupervisorEvent::Stopped => {}
        other => panic!("expected Stopped, got {other:?}"),
    }
    let status = supervisor.status().await;
    assert_eq!(status.state, dsh_supervisor::State::Stopped);

    let _ = std::fs::remove_file(&script);
}

#[tokio::test]
async fn crash_enters_backoff_then_relaunches() {
    // Exits immediately with code 1 and never prints the ready line.
    let script = write_script(
        "dsh-supervisor-test-crash.js",
        "console.error('boom'); process.exit(1)",
    );
    let supervisor = Supervisor::start(plan_for(&script), RestartPolicy::default());
    let mut receiver = supervisor.subscribe();

    match next_lifecycle(&mut receiver).await {
        SupervisorEvent::Starting { .. } => {}
        other => panic!("expected Starting, got {other:?}"),
    }
    match next_lifecycle(&mut receiver).await {
        SupervisorEvent::Crashed { attempt, reason, retry_in_ms } => {
            assert_eq!(attempt, 1);
            assert_eq!(retry_in_ms, 500);
            assert!(reason.contains("exit code"), "reason was {reason}");
        }
        other => panic!("expected Crashed, got {other:?}"),
    }
    match next_lifecycle(&mut receiver).await {
        SupervisorEvent::Starting { attempt } => assert_eq!(attempt, 2),
        other => panic!("expected second Starting, got {other:?}"),
    }

    // Stop now: it either lands during the child's short life (straight
    // Stopped) or during the second backoff (Crashed then Stopped). Both
    // are correct; the announcement must arrive either way.
    supervisor.stop();
    loop {
        match next_lifecycle(&mut receiver).await {
            SupervisorEvent::Stopped => break,
            SupervisorEvent::Crashed { attempt, .. } => assert_eq!(attempt, 2),
            other => panic!("unexpected {other:?} after stop"),
        }
    }

    let status = supervisor.status().await;
    assert_eq!(status.state, dsh_supervisor::State::Stopped);
    assert!(status.restarts >= 1);

    let _ = std::fs::remove_file(&script);
}

/// Regression for the black-screen failure: the daemon process stays alive
/// (hung npm wrapper) but its port stops accepting. The supervisor must
/// declare a crash via the liveness probe, not wait for process exit.
#[tokio::test]
async fn hung_wrapper_with_dead_port_is_declared_crashed() {
    let script = write_script(
        "dsh-supervisor-test-zombie.js",
        r#"
const server = require('http').createServer((req, res) => res.end('ok'))
server.listen(39124, '127.0.0.1', () => {
  console.log('dsh web: http://127.0.0.1:39124')
  setTimeout(() => {
    server.close(() => {})
    setInterval(() => {}, 60000) // keep the process alive after closing
  }, 2000)
})
"#,
    );
    let supervisor = Supervisor::start(plan_for(&script), RestartPolicy::default());
    let mut receiver = supervisor.subscribe();

    match next_lifecycle(&mut receiver).await {
        SupervisorEvent::Starting { .. } => {}
        other => panic!("expected Starting, got {other:?}"),
    }
    match next_lifecycle(&mut receiver).await {
        SupervisorEvent::Ready { port, .. } => assert_eq!(port, 39124),
        other => panic!("expected Ready, got {other:?}"),
    }

    // The port dies ~2s after ready; the probe declares death within a few
    // intervals. The child is then force-killed despite still "running".
    match next_lifecycle(&mut receiver).await {
        SupervisorEvent::Crashed { reason, .. } => {
            assert!(reason.contains("stopped responding"), "reason was {reason}");
        }
        other => panic!("expected ServiceDead Crashed, got {other:?}"),
    }

    supervisor.stop();
    loop {
        match next_lifecycle(&mut receiver).await {
            SupervisorEvent::Stopped => break,
            SupervisorEvent::Crashed { .. } => {}
            other => panic!("unexpected {other:?} after stop"),
        }
    }

    let _ = std::fs::remove_file(&script);
}
