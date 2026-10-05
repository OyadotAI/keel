//! Background commands Keel owns, so they outlive the turn that asked for them.
//!
//! Measured, twice: a turn is one `claude -p`, and the CLI kills every tracked background shell at
//! teardown. `gh run watch` started with `run_in_background` was `[killed]` eight seconds later,
//! and the person only found out six minutes on — from a `<task-notification>` saying `stopped`
//! that arrived when *they* typed again. The same kill is why "run it" needed running twice: the
//! agent's own `pnpm dev` died with the turn that started it.
//!
//! Nothing Keel puts on the command line changes that, so the job has to belong to the daemon
//! instead. The hook refuses the agent's own background shell and Keel runs the same command
//! here, in the lane's checkout, out of the turn's lifetime entirely. When it exits, the app
//! delivers the output back into that conversation as its own turn — which is the half that makes
//! "I'll watch it and report" a true sentence rather than a promise nobody kept.
//!
//! Kept deliberately small: this is a list of processes with their output, not a scheduler. There
//! is no cron here and no retry — a job runs once, and the person can stop it.

use crate::lock::Locked;
use crate::signals::Leads;
use axum::{Json, extract::Query};
use serde::{Deserialize, Serialize};
use std::process::Stdio;
use std::sync::{Mutex, OnceLock};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

/// Lines kept per job. A `tail -f` left running all day must not become a leak.
const MAX_LOG: usize = 400;
/// Finished jobs kept for the panel, oldest dropped first.
const KEEP_FINISHED: usize = 20;

/// One background command, as the app sees it.
#[derive(Serialize, Clone, Debug)]
pub struct Job {
    pub id: String,
    /// The conversation that asked for it, and the one its result goes back to.
    pub lane: String,
    pub command: String,
    /// Where it runs, shown so a lane's job is not confused with the project's.
    pub dir: String,
    /// Unix seconds, so the app can say "running for 4m" without a clock of its own.
    pub started: u64,
    pub finished: Option<u64>,
    pub exit: Option<i32>,
    pub log: Vec<String>,
    /// The first page on this machine the job announced — a dev server the agent started. Kept
    /// apart from the log, which shows the last lines only: the URL scrolled out of it after 80
    /// requests and the preview lost the server.
    #[serde(default)]
    pub url: Option<String>,
    /// Its completion has been handed to the conversation. Set by `ack`, so a job is never
    /// reported twice — not on a reconnect, and not by a second window polling the same lane.
    pub reported: bool,
}

impl Job {
    pub fn running(&self) -> bool {
        self.finished.is_none()
    }
}

struct Run {
    job: Job,
    /// The process group leader, for Stop. The `Child` itself belongs to the waiter task — one
    /// owner, so there is no way to `wait()` and `kill()` the same handle from two places.
    pid: u32,
}

fn jobs() -> &'static Mutex<Vec<Run>> {
    static J: OnceLock<Mutex<Vec<Run>>> = OnceLock::new();
    J.get_or_init(Default::default)
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

/// A page served on this machine: what the preview can show.
fn local_url(url: &str) -> bool {
    [
        "http://localhost",
        "http://127.0.0.1",
        "http://0.0.0.0",
        "http://[::1]",
        "https://localhost",
        "https://127.0.0.1",
    ]
    .iter()
    .any(|p| url.starts_with(p))
}

/// Start a command and return its id, or say why it could not start.
///
/// The id is short and readable because it goes into the conversation: the agent is told
/// "monitoring as job m3", and the person sees `m3` in the panel.
pub fn start(lane: &str, command: &str, dir: &camino::Utf8Path) -> Result<String, String> {
    let mut child = Command::new(crate::path::posix_shell())
        .arg("-c")
        .arg(command)
        .current_dir(dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Its own group, so Stop reaches whatever the command itself started. A `gh run watch`
        // piped into `tail` is two processes, and signalling only the shell leaves the other.
        .lead_group()
        .spawn()
        .map_err(|e| e.to_string())?;

    let pid = child.id().unwrap_or(0);
    let (out, err) = (child.stdout.take(), child.stderr.take());

    let id = {
        let mut all = jobs().locked();
        // Sequential rather than random: `m3` is something a person can say out loud, and the
        // count only ever climbs within one daemon. A counter, not the list's length: the list
        // is pruned, and `len() + 1` handed a new job an id a finished one still had.
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);
        let id = format!(
            "m{}",
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        all.push(Run {
            job: Job {
                id: id.clone(),
                lane: lane.to_string(),
                command: command.to_string(),
                dir: dir.to_string(),
                started: now(),
                finished: None,
                exit: None,
                log: Vec::new(),
                url: None,
                reported: false,
            },
            pid,
        });
        prune(&mut all);
        id
    };

    tokio::spawn(drain(id.clone(), out));
    tokio::spawn(drain(id.clone(), err));

    let waiting = id.clone();
    let lane_done = lane.to_string();
    tokio::spawn(async move {
        let code = child.wait().await.ok().and_then(|s| s.code()).unwrap_or(-1);
        // The command is done; whatever it started and left in its group is not a job any more,
        // and nothing could stop it. `a & b & wait` interrupted, or `server &` at the end of a
        // script, left processes listening after the job read "finished".
        if pid != 0 {
            crate::signals::group(pid, crate::signals::KILL);
        }
        {
            let mut all = jobs().locked();
            if let Some(r) = all.iter_mut().find(|r| r.job.id == waiting) {
                r.job.finished = Some(now());
                r.job.exit = Some(code);
            }
        }
        changed(&lane_done);
    });
    changed(lane);

    Ok(id)
}

/// Past this many finished jobs, undelivered ones go too. The exemption below is for a window
/// that has not asked yet, not for a daemon nobody ever asks — which is the case that grew
/// without bound.
const HARD_CEILING: usize = 200;

/// Keep the finished list bounded. Running jobs are never dropped.
fn prune(all: &mut Vec<Run>) {
    let finished = all.iter().filter(|r| !r.job.running()).count();
    if finished > HARD_CEILING {
        let mut over = finished - KEEP_FINISHED;
        all.retain(|r| {
            if r.job.running() || over == 0 {
                return true;
            }
            over -= 1;
            false
        });
        return;
    }
    if finished <= KEEP_FINISHED {
        return;
    }
    let mut over = finished - KEEP_FINISHED;
    all.retain(|r| {
        // An undelivered completion is not rubbish to make room with: dropping it is exactly
        // the silence this module exists to remove.
        if r.job.running() || !r.job.reported || over == 0 {
            return true;
        }
        over -= 1;
        false
    });
}

/// How often a job that is printing says so. The event used to go out per line, and every lane
/// of every window answered each one by fetching every job's log: a dev server that never goes
/// quiet was three full fetches a second per lane, forever. Once a second still moves the panel
/// while the job does, and a trailing one lands when it goes quiet.
const EVERY: std::time::Duration = std::time::Duration::from_secs(1);

/// The jobs changed, for `lane`. Carried so a window can ignore a lane it does not hold.
fn changed(lane: &str) {
    crate::events::emit(
        "monitors.changed",
        None,
        serde_json::json!({ "lane": lane }),
    );
}

async fn drain<R: tokio::io::AsyncRead + Unpin>(id: String, pipe: Option<R>) {
    let Some(pipe) = pipe else { return };
    let mut lines = BufReader::new(pipe).lines();
    let mut last: Option<tokio::time::Instant> = None;
    // A line arrived since the last event. `next_line` is cancel-safe, so waiting on it under a
    // deadline loses nothing.
    let mut owed: Option<String> = None;
    // Undecodable bytes are not EOF: stopping there leaves the pipe to fill, and a full pipe
    // blocks the job mid-write. Same lesson as the agent's own stderr drain, and the same helper,
    // which is also what keeps a genuinely broken reader from becoming a busy loop.
    loop {
        let next = match (&owed, last) {
            (Some(lane), Some(at)) => {
                match tokio::time::timeout_at(at + EVERY, crate::lines::next(&mut lines)).await {
                    Ok(next) => next,
                    Err(_) => {
                        changed(lane);
                        last = Some(tokio::time::Instant::now());
                        owed = None;
                        continue;
                    }
                }
            }
            _ => crate::lines::next(&mut lines).await,
        };
        match next {
            crate::lines::Next::Line(line) => {
                let mut all = jobs().locked();
                let Some(r) = all.iter_mut().find(|r| r.job.id == id) else {
                    return;
                };
                if r.job.url.is_none() {
                    r.job.url = crate::dev::find_url(&line).filter(|u| local_url(u));
                }
                r.job.log.push(crate::dev::capped(line));
                let len = r.job.log.len();
                if len > MAX_LOG {
                    r.job.log.drain(..len - MAX_LOG);
                }
                let lane = r.job.lane.clone();
                drop(all);
                if last.is_none_or(|at| at.elapsed() >= EVERY) {
                    changed(&lane);
                    last = Some(tokio::time::Instant::now());
                    owed = None;
                } else {
                    owed = Some(lane);
                }
            }
            crate::lines::Next::Skipped => continue,
            crate::lines::Next::Done => {
                if let Some(lane) = owed {
                    changed(&lane);
                }
                return;
            }
        }
    }
}

/// How much of a job's output travels. The app shows the last 40 lines in the panel and hands the
/// last 80 to the conversation when the job finishes, so anything past this is copied out of the
/// lock, serialised, sent and decoded on the main actor every two seconds in order to be dropped.
/// `MAX_LOG` is what is *kept*; this is what is *shown*.
const SHOWN: usize = 80;

/// The jobs belonging to one conversation, newest first. Without a lane, every job — which is
/// what a fresh window asks for before it has an id of its own.
///
/// A job with no lane of its own belongs to whoever asks, which is the rule `Pending` already
/// keeps for questions. The asymmetry was a way to be invisible: the lane on a job is the one on
/// the hook's command line, a spawn that carried none filed the job under `""`, and a window
/// asking by its own id then matched nothing — a background command running, listed nowhere, with
/// the agent's reply saying Keel was watching it. Something running that nothing shows is the one
/// outcome this whole file exists to prevent.
pub fn list(lane: Option<&str>) -> Vec<Job> {
    let all = jobs().locked();
    let mut out: Vec<Job> = all
        .iter()
        .filter(|r| belongs(&r.job.lane, lane))
        .map(|r| {
            let mut job = r.job.clone();
            let len = job.log.len();
            if len > SHOWN {
                job.log.drain(..len - SHOWN);
            }
            job
        })
        .collect();
    out.reverse();
    out
}

/// Whether a job is one this caller should see.
///
/// Its own, and anything filed under no lane at all.
fn belongs(job: &str, asked: Option<&str>) -> bool {
    asked.is_none_or(|l| l.is_empty() || job.is_empty() || job == l)
}

#[derive(Deserialize)]
pub struct LaneQuery {
    #[serde(default)]
    pub lane: Option<String>,
}

pub async fn api_list(Query(q): Query<LaneQuery>) -> Json<Vec<Job>> {
    // no-blocking: the registry is memory.
    Json(list(q.lane.as_deref()))
}

#[derive(Deserialize)]
pub struct IdBody {
    pub id: String,
}

/// Stop a job. SIGINT to the group, not SIGTERM — the same choice as stopping a turn, and for the
/// same reason: an interrupt lets what is running unwind and print why it ended.
pub async fn api_stop(Json(body): Json<IdBody>) -> Json<bool> {
    // no-blocking: a signal and memory.
    let pid = {
        let all = jobs().locked();
        all.iter()
            .find(|r| r.job.id == body.id && r.job.running())
            .map(|r| r.pid)
    };
    let Some(pid) = pid.filter(|p| *p != 0) else {
        return Json(false);
    };
    // Interrupted first, so it can unwind and say why it ended; then the group is ended. `&`
    // children of a non-interactive `sh` ignore SIGINT, and so does anything that traps it — Stop
    // said true and both servers in `a & b & wait` kept listening.
    crate::signals::group(pid, crate::signals::INTERRUPT);
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        crate::signals::group(pid, crate::signals::KILL);
    });
    Json(true)
}

/// Kill every running job. Called when the daemon is about to exit with its app.
///
/// SIGKILL rather than the SIGINT `api_stop` sends, and the difference is the point: Stop is a
/// person interrupting something they want to see the end of, and this is the process going away.
/// A monitored `pnpm dev` that catches SIGINT and takes its time would be reparented to init and
/// serve on the same port forever, which is precisely the orphan the app's own budget forbids.
pub fn stop_all() {
    let all = jobs().locked();
    // Every job's group, finished or not: a finished job's leftovers are exactly what outlived
    // the app before this.
    for r in all.iter().filter(|r| r.pid != 0) {
        crate::signals::group(r.pid, crate::signals::KILL);
    }
}

/// Mark a completion as delivered to the conversation.
pub async fn api_ack(Json(body): Json<IdBody>) -> Json<bool> {
    // no-blocking: the registry is memory.
    let mut all = jobs().locked();
    // `true` to the first ack only. Two reads of the list can both see a job unreported, and an
    // ack that said yes to both put its result into the conversation twice.
    match all.iter_mut().find(|r| r.job.id == body.id) {
        Some(r) if !r.job.reported => {
            r.job.reported = true;
            Json(true)
        }
        _ => Json(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // No `clear()` here on purpose: the registry is global and the test binary is threaded, so
    // wiping it is how one test deletes another's job. Every test uses lanes of its own instead.

    fn pids_in(id: &str) -> Vec<i32> {
        let all = jobs().locked();
        all.iter()
            .find(|r| r.job.id == id)
            .map(|r| {
                r.job
                    .log
                    .iter()
                    .filter_map(|l| l.trim().parse().ok())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn alive(pid: i32) -> bool {
        unsafe { libc::kill(pid, 0) == 0 }
    }

    async fn settle(pids: &[i32]) -> bool {
        for _ in 0..100 {
            if pids.iter().all(|p| !alive(*p)) {
                return true;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        for p in pids {
            unsafe { libc::kill(*p, libc::SIGKILL) };
        }
        false
    }

    /// `&` children of `sh -c` ignore SIGINT: Stop said true and both kept running.
    #[cfg(unix)]
    #[tokio::test]
    async fn stop_ends_what_the_job_put_in_the_background() {
        let dir = tempfile::tempdir().unwrap();
        let here = camino::Utf8Path::from_path(dir.path()).unwrap();
        let id = start(
            "stop-bg",
            "sleep 300 & echo $!; sleep 301 & echo $!; wait",
            here,
        )
        .unwrap();
        let mut pids = Vec::new();
        for _ in 0..100 {
            pids = pids_in(&id);
            if pids.len() == 2 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(pids.len(), 2, "the job did not report its children");
        assert!(api_stop(Json(IdBody { id })).await.0);
        assert!(
            settle(&pids).await,
            "Stop left the job's background processes running"
        );
    }

    /// The command ended; what it left behind ends with it.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_finished_job_leaves_nothing_running() {
        let dir = tempfile::tempdir().unwrap();
        let here = camino::Utf8Path::from_path(dir.path()).unwrap();
        let id = start("leftovers", "sleep 300 & echo $!", here).unwrap();
        let mut pids = Vec::new();
        for _ in 0..100 {
            pids = pids_in(&id);
            if !pids.is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(pids.len(), 1);
        assert!(
            settle(&pids).await,
            "a finished job left its background process running"
        );
    }

    #[test]
    fn a_job_keeps_the_url_it_announced_and_lines_are_capped() {
        assert!(local_url("http://localhost:5173"));
        assert!(!local_url("https://example.com"));
        assert_eq!(
            crate::dev::find_url("  ➜  Local:   http://localhost:\u{1b}[1m5173\u{1b}[22m/")
                .as_deref(),
            Some("http://localhost:5173")
        );
        assert!(crate::dev::capped("x".repeat(2_000_000)).len() < 5000);
    }

    /// The whole point: the command outlives the call that started it, and its output is here
    /// afterwards rather than in a file nobody reads.
    #[tokio::test]
    async fn a_job_runs_past_the_call_that_started_it_and_keeps_its_output() {
        let dir = camino::Utf8PathBuf::from("/tmp");
        let id = start("lane-1", "echo hello; sleep 0.2; echo bye", &dir).expect("started");
        // Still running when `start` returned, which is what "background" has to mean.
        assert!(list(Some("lane-1"))[0].running());
        for _ in 0..100 {
            if !list(Some("lane-1"))[0].running() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let job = list(Some("lane-1")).into_iter().next().expect("the job");
        assert_eq!(job.id, id);
        assert_eq!(job.exit, Some(0));
        assert_eq!(job.log, vec!["hello".to_string(), "bye".to_string()]);
    }

    /// A completion is delivered to the conversation once. Polling twice, or a second window
    /// polling the same lane, must not put the same result into the chat again.
    #[tokio::test]
    async fn a_completion_is_only_reported_once() {
        let id = start("lane-2", "true", &camino::Utf8PathBuf::from("/tmp")).expect("started");
        assert!(!list(Some("lane-2"))[0].reported);
        assert!(
            api_ack(Json(IdBody { id: id.clone() })).await.0,
            "the first ack delivers"
        );
        assert!(list(Some("lane-2"))[0].reported);
        assert!(
            !api_ack(Json(IdBody { id })).await.0,
            "a second ack does not"
        );
    }

    /// A job nobody claimed is seen by whoever asks. Filed under `""` — a spawn whose hook
    /// carried no lane — it used to match no window at all, which is a background command
    /// running, listed nowhere, with the agent's reply saying Keel was watching it.
    ///
    /// Asserted on the rule rather than through `start`, because the job store is process-wide
    /// and a job with no lane is one every other test in this file would then see.
    #[test]
    fn a_job_with_no_lane_of_its_own_is_visible_from_one() {
        assert!(belongs("", Some("lane-a")));
        assert!(belongs("lane-a", Some("lane-a")));
        assert!(!belongs("lane-b", Some("lane-a")));
        assert!(
            belongs("lane-b", None),
            "a window with no id yet sees everything"
        );
        assert!(belongs("lane-b", Some("")));
    }

    /// A lane sees its own jobs and nobody else's — the same rule as questions.
    #[tokio::test]
    async fn jobs_belong_to_one_conversation() {
        let dir = camino::Utf8PathBuf::from("/tmp");
        start("lane-a", "true", &dir).expect("started");
        start("lane-b", "true", &dir).expect("started");
        assert_eq!(list(Some("lane-a")).len(), 1);
        assert_eq!(list(Some("lane-b")).len(), 1);
        assert!(list(Some("lane-a")).iter().all(|j| j.lane == "lane-a"));
    }

    /// A job that prints is not an event per line. Every lane of every window answered each one
    /// by fetching every job's log, so a dev server was three full fetches a second per lane for
    /// as long as it ran. Two hundred lines in a burst must come out as a handful of events, and
    /// the last of them must still arrive after the job goes quiet.
    #[tokio::test]
    async fn a_chatty_job_is_a_handful_of_events_not_one_per_line() {
        let mut events = crate::events::subscribe();
        let dir = camino::Utf8PathBuf::from("/tmp");
        start("lane-chatty", "seq 1 200; sleep 1.5", &dir).expect("started");
        let mut ours = 0;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(4);
        while let Ok(Ok(e)) = tokio::time::timeout_at(deadline, events.recv()).await {
            if e.kind == "monitors.changed" && e.data["lane"] == "lane-chatty" {
                ours += 1;
            }
        }
        let job = list(Some("lane-chatty"))
            .into_iter()
            .next()
            .expect("the job");
        assert_eq!(job.log.last().map(String::as_str), Some("200"));
        assert!(!job.running());
        // Start, the first line, one trailing edge (stdout), finish. Allow some slack for stderr
        // and scheduling, but nowhere near two hundred.
        assert!((2..=8).contains(&ours), "{ours} events for 200 lines");
    }
}
