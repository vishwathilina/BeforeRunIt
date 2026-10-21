//! Background daemon. Answers zsh queries over a Unix socket, runs the
//! debounced AI review for every shell, caches verdicts and animates the
//! ghost line (zsh: pushed frames; bash/fish: careful writes to the tty).
//! It sleeps completely when nobody is typing.

use std::collections::HashMap;
use std::io::Write as _;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use beforerunit_ai::{redact, Client, ReviewRequest};
use beforerunit_core::config::{AiMode, DisplayMode};
use beforerunit_core::ghost::{build_view, AiStatus};
use beforerunit_core::merge::merge;
use beforerunit_core::painter::paint_below;
use beforerunit_core::proto::{self, escape, unescape, Caps};
use beforerunit_core::render::{self, View};
use beforerunit_core::store;
use beforerunit_core::{Analysis, Engine, Level};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::cache::{self, Cache};
use crate::ui::Ui;

const ZSH_FRAME: Duration = Duration::from_millis(83);
const TTY_FRAME: Duration = Duration::from_millis(125);
/// Stop animating this long after the last keystroke (the final frame stays).
const ANIM_IDLE: Duration = Duration::from_secs(4);
/// Lets the shell print its own Tier 1 frame before the daemon repaints.
const TTY_SETTLE: Duration = Duration::from_millis(25);
const IDLE_EXIT: Duration = Duration::from_secs(3 * 3600);
const MAX_LINE: usize = 64 * 1024;

#[derive(Clone)]
enum Kind {
    Zsh,
    Tty { tty: String, pid: String, down: u16 },
}

struct Session {
    engine: Engine,
    gen: u64,
    seq: String,
    line: String,
    fresh: bool,
    cwd: String,
    root: bool,
    cols: usize,
    caps: Caps,
    mode: String,
    kind: Kind,
    analysis: Analysis,
    ai: AiStatus,
    cancel: Option<CancellationToken>,
    push: Option<mpsc::UnboundedSender<String>>,
    frame: u32,
    changed_at: Option<u32>,
    anim: bool,
    touched: Instant,
}

impl Session {
    fn new() -> Session {
        Session {
            engine: Engine::new(),
            gen: 0,
            seq: "0".into(),
            line: String::new(),
            fresh: false,
            cwd: String::new(),
            root: false,
            cols: 80,
            caps: Caps { depth: render::ColorDepth::True, italic: true },
            mode: String::new(),
            kind: Kind::Zsh,
            analysis: Analysis::default(),
            ai: AiStatus::Off,
            cancel: None,
            push: None,
            frame: 0,
            changed_at: None,
            anim: false,
            touched: Instant::now(),
        }
    }

    fn stop(&mut self) {
        if let Some(t) = self.cancel.take() {
            t.cancel();
        }
        self.anim = false;
        self.ai = AiStatus::Off;
        self.line.clear();
        self.fresh = false;
        self.changed_at = None;
    }
}

#[derive(Default)]
struct Stats {
    queries: AtomicU64,
    notifies: AtomicU64,
    ai_calls: AtomicU64,
    ai_cache_hits: AtomicU64,
    ai_errors: AtomicU64,
    tty_writes: AtomicU64,
}

struct State {
    ui: RwLock<Arc<Ui>>,
    ai: RwLock<Option<Arc<Client>>>,
    cache: Cache,
    sessions: Mutex<HashMap<String, Session>>,
    stats: Stats,
    started: Instant,
    activity: Mutex<Instant>,
    fresh_check: Mutex<Instant>,
    shutdown: CancellationToken,
}

impl State {
    fn ui(&self) -> Arc<Ui> {
        self.ui.read().unwrap().clone()
    }

    fn client(&self) -> Option<Arc<Client>> {
        self.ai.read().unwrap().clone()
    }

    /// Picks up config edits without needing RELOAD, at most every 2 s.
    fn check_fresh(&self) {
        {
            let mut t = self.fresh_check.lock().unwrap();
            if t.elapsed() < Duration::from_secs(2) {
                return;
            }
            *t = Instant::now();
        }
        if !store::is_fresh(&self.ui().compiled) {
            self.reload();
        }
    }

    fn reload(&self) {
        let ui = Ui::load();
        let client = build_client(&ui);
        *self.ui.write().unwrap() = Arc::new(ui);
        *self.ai.write().unwrap() = client.clone();
        for s in self.sessions.lock().unwrap().values_mut() {
            s.engine.reset();
            s.fresh = false;
        }
        if let Some(c) = client {
            tokio::spawn(async move { c.warm().await });
        }
    }
}

fn build_client(ui: &Ui) -> Option<Arc<Client>> {
    let cfg = &ui.config().ai;
    if cfg.effective_mode() == AiMode::Off {
        return None;
    }
    Client::new(crate::context::endpoint(cfg, crate::keys::api_key())).ok().map(Arc::new)
}

// ---------------------------------------------------------------------------
// Start / control

pub fn start(foreground: bool) -> i32 {
    if proto::ping().is_some() {
        if foreground {
            eprintln!("beforerunit: daemon already running");
        }
        return 0;
    }
    if !foreground {
        return spawn_background();
    }
    match run_foreground() {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("beforerunit daemon: {e}");
            1
        }
    }
}

fn spawn_background() -> i32 {
    use std::os::unix::process::CommandExt;
    let exe = std::env::current_exe().unwrap_or_else(|_| "beforerunit".into());
    let log = store::cache_dir().join("daemon.log");
    let _ = std::fs::create_dir_all(store::cache_dir());
    let out = std::fs::OpenOptions::new().create(true).append(true).open(&log).ok();
    let mut cmd = std::process::Command::new(exe);
    cmd.args(["daemon", "--foreground"]).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null());
    match out {
        Some(f) => cmd.stderr(f),
        None => cmd.stderr(std::process::Stdio::null()),
    };
    // SAFETY: setsid only detaches the child from the terminal session.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    if cmd.spawn().is_err() {
        eprintln!("beforerunit: cannot start the daemon");
        return 1;
    }
    for _ in 0..30 {
        std::thread::sleep(Duration::from_millis(50));
        if proto::ping().is_some() {
            return 0;
        }
    }
    eprintln!("beforerunit: daemon did not come up, see {}", log.display());
    1
}

pub fn ctl(cmd: &str) -> i32 {
    match proto::request(cmd, Duration::from_secs(5)) {
        Some(r) => {
            if let Some(json) = r.strip_prefix("STATS\t") {
                match serde_json::from_str::<serde_json::Value>(json) {
                    Ok(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default()),
                    Err(_) => println!("{json}"),
                }
            } else {
                println!("{r}");
            }
            0
        }
        None => {
            if cmd == "STOP" {
                println!("Daemon is not running.");
                0
            } else {
                eprintln!("beforerunit: daemon is not running (start it with `beforerunit daemon`)");
                1
            }
        }
    }
}

/// Holds an exclusive lock next to the socket so two daemons never race for it.
fn lock_instance() -> std::io::Result<std::fs::File> {
    let sock = store::socket_path();
    let dir = sock.parent().map(|p| p.to_path_buf()).unwrap_or_else(store::runtime_dir);
    std::fs::create_dir_all(&dir)?;
    if dir.starts_with("/tmp/beforerunit-") {
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    let f = std::fs::OpenOptions::new().create(true).truncate(false).write(true).mode(0o600).open(sock.with_extension("lock"))?;
    // SAFETY: flock on a file descriptor we own.
    if unsafe { libc::flock(std::os::fd::AsRawFd::as_raw_fd(&f), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(std::io::Error::other("another daemon holds the lock"));
    }
    Ok(f)
}

fn run_foreground() -> anyhow::Result<()> {
    let _lock = lock_instance()?;
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).thread_name("beforerunit").enable_all().build()?;
    rt.block_on(serve())
}

async fn serve() -> anyhow::Result<()> {
    let path = store::socket_path();
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;

    let ui = Ui::load();
    let client = build_client(&ui);
    let st = Arc::new(State {
        ui: RwLock::new(Arc::new(ui)),
        ai: RwLock::new(client.clone()),
        cache: Cache::open(),
        sessions: Mutex::new(HashMap::new()),
        stats: Stats::default(),
        started: Instant::now(),
        activity: Mutex::new(Instant::now()),
        fresh_check: Mutex::new(Instant::now()),
        shutdown: CancellationToken::new(),
    });
    if let Some(c) = client {
        tokio::spawn(async move { c.warm().await });
    }
    eprintln!("beforerunit daemon {} listening on {}", store::VERSION, path.display());

    tokio::spawn(watchdog(st.clone()));
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    loop {
        tokio::select! {
            r = listener.accept() => {
                if let Ok((stream, _)) = r {
                    tokio::spawn(connection(st.clone(), stream));
                }
            }
            _ = st.shutdown.cancelled() => break,
            _ = term.recv() => break,
            _ = tokio::signal::ctrl_c() => break,
        }
    }
    // Only remove the socket if it is still ours.
    if proto::ping().is_none() {
        let _ = std::fs::remove_file(&path);
    }
    Ok(())
}

/// Wakes rarely: drops stale sessions and exits after long inactivity.
async fn watchdog(st: Arc<State>) {
    loop {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(600)) => {}
            _ = st.shutdown.cancelled() => return,
        }
        st.sessions.lock().unwrap().retain(|_, s| s.touched.elapsed() < Duration::from_secs(24 * 3600) || s.push.is_some());
        let idle = st.activity.lock().unwrap().elapsed();
        let any_sub = st.sessions.lock().unwrap().values().any(|s| s.push.is_some());
        if idle > IDLE_EXIT && !any_sub {
            eprintln!("beforerunit daemon: idle, exiting");
            st.shutdown.cancel();
        }
    }
}

// ---------------------------------------------------------------------------
// Connections

async fn connection(st: Arc<State>, stream: UnixStream) {
    if stream.peer_cred().map(|c| c.uid() != store::uid()).unwrap_or(true) {
        return;
    }
    let (r, mut w) = stream.into_split();
    let mut lines = BufReader::new(r).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.len() > MAX_LINE {
            continue;
        }
        *st.activity.lock().unwrap() = Instant::now();
        if let Some(sid) = line.strip_prefix("S\t") {
            let sid = unescape(sid);
            let (tx, mut rx) = mpsc::unbounded_channel::<String>();
            st.sessions.lock().unwrap().entry(sid.clone()).or_insert_with(Session::new).push = Some(tx);
            loop {
                tokio::select! {
                    m = rx.recv() => match m {
                        Some(m) => if w.write_all(m.as_bytes()).await.is_err() { break },
                        None => break,
                    },
                    l = lines.next_line() => match l {
                        Ok(Some(l)) => { let _ = handle(&st, &l); }
                        _ => break,
                    },
                    _ = st.shutdown.cancelled() => break,
                }
            }
            if let Some(s) = st.sessions.lock().unwrap().get_mut(&sid) {
                s.push = None;
                s.anim = false;
            }
            return;
        }
        if let Some(reply) = handle(&st, &line) {
            if w.write_all(reply.as_bytes()).await.is_err() {
                return;
            }
        }
    }
}

fn handle(st: &Arc<State>, line: &str) -> Option<String> {
    let f: Vec<&str> = line.split('\t').collect();
    match f[0] {
        "Q" if f.len() >= 9 => {
            st.stats.queries.fetch_add(1, Ordering::Relaxed);
            st.check_fresh();
            let sid = unescape(f[1]);
            let mut sessions = st.sessions.lock().unwrap();
            let s = sessions.entry(sid.clone()).or_insert_with(Session::new);
            s.seq = f[2].to_string();
            s.cols = f[3].parse().unwrap_or(80);
            s.caps = Caps::parse(f[4]);
            s.mode = unescape(f[5]);
            s.cwd = unescape(f[6]);
            s.root = f[7] == "0";
            s.kind = Kind::Zsh;
            update(st, &sid, s, &unescape(f[8]));
            let ui = st.ui();
            let (text, specs) = zsh_frame(&ui, s);
            let reply = format!("R\t{}\t{}\t{}\n", s.seq, escape(&text), escape(&specs));
            ensure_anim(st, &sid, s, &ui);
            Some(reply)
        }
        "N" if f.len() >= 12 => {
            st.stats.notifies.fetch_add(1, Ordering::Relaxed);
            st.check_fresh();
            let sid = unescape(f[1]);
            let mut sessions = st.sessions.lock().unwrap();
            let s = sessions.entry(sid.clone()).or_insert_with(Session::new);
            s.seq = f[2].to_string();
            s.cols = f[3].parse().unwrap_or(80);
            s.caps = Caps::parse(f[4]);
            s.mode = unescape(f[5]);
            s.cwd = unescape(f[6]);
            s.root = f[7] == "0";
            s.kind = Kind::Tty { tty: unescape(f[8]), pid: unescape(f[9]), down: f[10].parse().unwrap_or(1) };
            let line = unescape(f[11]);
            let gen_before = s.gen;
            update(st, &sid, s, &line);
            if matches!(s.ai, AiStatus::Done(..)) && s.gen != gen_before {
                schedule_tty(st.clone(), sid.clone(), s.gen);
            }
            let ui = st.ui();
            ensure_anim(st, &sid, s, &ui);
            None
        }
        "X" if f.len() >= 2 => {
            if let Some(s) = st.sessions.lock().unwrap().get_mut(&unescape(f[1])) {
                s.stop();
                s.gen += 1;
            }
            None
        }
        "E" if f.len() >= 2 => {
            if let Some(mut s) = st.sessions.lock().unwrap().remove(&unescape(f[1])) {
                s.stop();
            }
            None
        }
        "PING" => Some(format!("PONG\t{}\t{}\n", store::VERSION, st.sessions.lock().unwrap().len())),
        "RELOAD" => {
            st.reload();
            Some("OK\n".into())
        }
        "CLEARCACHE" => {
            st.cache.clear();
            Some("OK\n".into())
        }
        "STATS" => Some(format!("STATS\t{}\n", stats_json(st))),
        "STOP" => {
            st.shutdown.cancel();
            Some("OK stopping\n".into())
        }
        _ => Some("ERR unknown message\n".into()),
    }
}

fn rss_kb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("VmRSS:")).and_then(|l| l.split_whitespace().nth(1)?.parse().ok()))
        .unwrap_or(0)
}

fn stats_json(st: &State) -> String {
    let ui = st.ui();
    let ai = &ui.config().ai;
    let s = &st.stats;
    serde_json::json!({
        "version": store::VERSION,
        "uptime_s": st.started.elapsed().as_secs(),
        "rss_kb": rss_kb(),
        "sessions": st.sessions.lock().unwrap().len(),
        "queries": s.queries.load(Ordering::Relaxed),
        "notifies": s.notifies.load(Ordering::Relaxed),
        "ai_mode": ai.effective_mode().as_str(),
        "ai_model": ai.model,
        "ai_calls": s.ai_calls.load(Ordering::Relaxed),
        "ai_cache_hits": s.ai_cache_hits.load(Ordering::Relaxed),
        "ai_errors": s.ai_errors.load(Ordering::Relaxed),
        "ai_cache_entries": st.cache.len(),
        "tty_writes": s.tty_writes.load(Ordering::Relaxed),
    })
    .to_string()
}

// ---------------------------------------------------------------------------
// Analysis and AI review

fn display_mode(ui: &Ui, s: &Session) -> DisplayMode {
    DisplayMode::parse(&s.mode).unwrap_or(ui.config().ui.display)
}

fn cwd_name(cwd: &str) -> &str {
    cwd.trim_end_matches('/').rsplit('/').next().unwrap_or("")
}

fn update(st: &Arc<State>, sid: &str, s: &mut Session, line: &str) {
    s.touched = Instant::now();
    if s.fresh && s.line == line {
        return;
    }
    s.fresh = true;
    s.line.clear();
    s.line.push_str(line);
    s.analysis = s.engine.analyze(line).clone();
    s.gen += 1;
    s.changed_at = None;
    if let Some(t) = s.cancel.take() {
        t.cancel();
    }
    let ui = st.ui();
    let cfg = &ui.config().ai;
    let wants = match cfg.effective_mode() {
        AiMode::Off => false,
        AiMode::UnknownOnly => s.analysis.needs_ai(),
        AiMode::ReviewAll => !s.analysis.is_empty() && s.analysis.level != Level::Idle,
    } && display_mode(&ui, s) != DisplayMode::Off;
    let Some(client) = st.client().filter(|_| wants) else {
        s.ai = AiStatus::Off;
        return;
    };
    let red = redact(line);
    let key = cache::key(&red, cwd_name(&s.cwd), s.root, &cfg.model);
    if let Some(v) = st.cache.get_mem(key) {
        st.stats.ai_cache_hits.fetch_add(1, Ordering::Relaxed);
        s.ai = AiStatus::Done(merge(&s.analysis, &v, cfg.min_confidence_to_lower), v);
        return;
    }
    s.ai = AiStatus::Pending;
    let token = CancellationToken::new();
    s.cancel = Some(token.clone());
    let job = Job {
        sid: sid.to_string(),
        gen: s.gen,
        key,
        command: red,
        cwd: s.cwd.clone(),
        root: s.root,
        targets: s.analysis.targets.clone(),
        tier1_level: s.analysis.level,
        tier1_text: s.analysis.text.clone(),
        debounce: Duration::from_millis(cfg.debounce_ms.min(5000)),
        min_conf: cfg.min_confidence_to_lower,
    };
    tokio::spawn(review(st.clone(), client, job, token));
}

struct Job {
    sid: String,
    gen: u64,
    key: u64,
    command: String,
    cwd: String,
    root: bool,
    targets: Vec<String>,
    tier1_level: Level,
    tier1_text: String,
    debounce: Duration,
    min_conf: f32,
}

async fn review(st: Arc<State>, client: Arc<Client>, job: Job, token: CancellationToken) {
    tokio::select! {
        _ = token.cancelled() => return,
        _ = tokio::time::sleep(job.debounce) => {}
    }
    let verdict = match st.cache.get(job.key) {
        Some(v) => {
            st.stats.ai_cache_hits.fetch_add(1, Ordering::Relaxed);
            Ok(v)
        }
        None => {
            let ctx = crate::context::gather(&job.cwd, job.root, &job.targets).await;
            if token.is_cancelled() {
                return;
            }
            st.stats.ai_calls.fetch_add(1, Ordering::Relaxed);
            let req = ReviewRequest { command: &job.command, tier1_level: job.tier1_level, tier1_text: &job.tier1_text, context: &ctx };
            let r = tokio::select! {
                _ = token.cancelled() => return,
                r = client.review(&req) => r,
            };
            if let Ok(v) = &r {
                st.cache.put(job.key, v);
            }
            r
        }
    };
    let mut sessions = st.sessions.lock().unwrap();
    let Some(s) = sessions.get_mut(&job.sid) else { return };
    if s.gen != job.gen {
        return;
    }
    s.ai = match verdict {
        Ok(v) => {
            let m = merge(&s.analysis, &v, job.min_conf);
            if m.changed {
                s.changed_at = Some(s.frame);
            }
            AiStatus::Done(m, v)
        }
        Err(e) => {
            st.stats.ai_errors.fetch_add(1, Ordering::Relaxed);
            eprintln!("beforerunit daemon: AI review failed: {}", e.explain());
            AiStatus::Error(e.short().into())
        }
    };
    let ui = st.ui();
    deliver(&st, &ui, s);
    ensure_anim(&st, &job.sid, s, &ui);
}

// ---------------------------------------------------------------------------
// Frames and delivery

fn view(ui: &Ui, s: &Session) -> Option<View> {
    let mut v = build_view(&s.analysis, &s.ai, display_mode(ui, s))?;
    v.changed_frames = s.changed_at.map(|c| s.frame.saturating_sub(c));
    Some(v)
}

fn zsh_frame(ui: &Ui, s: &Session) -> (String, String) {
    match view(ui, s) {
        Some(v) => {
            let styled = ui.render(&v, s.caps.depth, s.frame, s.cols, true);
            render::to_zsh(&styled, s.caps.depth, s.caps.italic)
        }
        None => (String::new(), String::new()),
    }
}

fn deliver(st: &State, ui: &Ui, s: &mut Session) {
    match &s.kind {
        Kind::Zsh => {
            if let Some(tx) = &s.push {
                let (text, specs) = zsh_frame(ui, s);
                let _ = tx.send(format!("R\t{}\t{}\t{}\n", s.seq, escape(&text), escape(&specs)));
            }
        }
        Kind::Tty { tty, pid, down } => {
            let ansi = match view(ui, s) {
                Some(v) => render::to_ansi(&ui.render(&v, s.caps.depth, s.frame, s.cols, true), s.caps.depth),
                None => String::new(),
            };
            if write_tty(tty, pid, &paint_below(&ansi, *down, false)) {
                st.stats.tty_writes.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

/// Delivers to a tty session after the shell has printed its own frame.
fn schedule_tty(st: Arc<State>, sid: String, gen: u64) {
    tokio::spawn(async move {
        tokio::time::sleep(TTY_SETTLE).await;
        let ui = st.ui();
        if let Some(s) = st.sessions.lock().unwrap().get_mut(&sid) {
            if s.gen == gen {
                deliver(&st, &ui, s);
            }
        }
    });
}

fn animating(ui: &Ui, s: &Session) -> bool {
    let Some(v) = view(ui, s) else { return false };
    if s.touched.elapsed() > ANIM_IDLE {
        return false;
    }
    match s.kind {
        Kind::Zsh if s.push.is_none() => false,
        _ => render::is_animated(&v, &ui.opts(s.caps.depth, s.frame, s.cols, true)),
    }
}

fn ensure_anim(st: &Arc<State>, sid: &str, s: &mut Session, ui: &Ui) {
    if s.anim || !animating(ui, s) {
        return;
    }
    s.anim = true;
    let period = if matches!(s.kind, Kind::Zsh) { ZSH_FRAME } else { TTY_FRAME };
    let (st, sid) = (st.clone(), sid.to_string());
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(period);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        tick.tick().await;
        loop {
            tokio::select! {
                _ = tick.tick() => {}
                _ = st.shutdown.cancelled() => return,
            }
            let ui = st.ui();
            let mut sessions = st.sessions.lock().unwrap();
            let Some(s) = sessions.get_mut(&sid) else { return };
            if !s.anim {
                return;
            }
            s.frame = s.frame.wrapping_add(1);
            let keep = animating(&ui, s);
            deliver(&st, &ui, s);
            if !keep {
                s.anim = false;
                return;
            }
        }
    });
}

/// Writes to the user's terminal only while their shell owns the foreground,
/// so a running command's output is never touched.
fn write_tty(tty: &str, pid: &str, data: &str) -> bool {
    if !(tty.starts_with("/dev/pts/") || tty.starts_with("/dev/tty")) || tty.contains("..") {
        return false;
    }
    if !shell_in_foreground(pid) {
        return false;
    }
    let Ok(meta) = std::fs::metadata(tty) else { return false };
    if std::os::unix::fs::MetadataExt::uid(&meta) != store::uid() {
        return false;
    }
    let Ok(mut f) = std::fs::OpenOptions::new().write(true).custom_flags(libc::O_NOCTTY | libc::O_NONBLOCK).open(tty) else {
        return false;
    };
    f.write_all(data.as_bytes()).is_ok()
}

fn shell_in_foreground(pid: &str) -> bool {
    if pid.is_empty() || !pid.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default();
    if !matches!(comm.trim(), "bash" | "fish" | "sh") {
        return false;
    }
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else { return false };
    let Some(rest) = stat.rfind(')').map(|i| &stat[i + 1..]) else { return false };
    // state ppid pgrp session tty_nr tpgid
    let f: Vec<&str> = rest.split_whitespace().collect();
    f.len() > 5 && f[2] == f[5]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn foreground_check_rejects_garbage() {
        assert!(!shell_in_foreground(""));
        assert!(!shell_in_foreground("1;rm"));
        assert!(!write_tty("/etc/passwd", "1", "x"));
    }

    #[test]
    fn cwd_names() {
        assert_eq!(cwd_name("/home/a/proj/"), "proj");
        assert_eq!(cwd_name("/"), "");
    }
}
