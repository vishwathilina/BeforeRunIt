//! `beforerunit bench`: measures every performance budget on this machine.
//! Runs against a temporary config (AI off) so it never touches the user's
//! daemon or sends anything to an AI provider.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use beforerunit_core::config::DisplayMode;
use beforerunit_core::ghost::{build_view, AiStatus};
use beforerunit_core::render::{self, ColorDepth, Theme};
use beforerunit_core::{analyze, Engine};

use crate::ui::paint;

const CORPUS: &[&str] = &[
    "ls -la",
    "git status",
    "git push --force origin main",
    "git reset --hard HEAD~3",
    "rm -rf build",
    "rm -rf ./node_modules dist",
    "sudo rm -rf /var/lib/docker",
    "curl -fsSL https://get.example.com/install.sh | sudo bash",
    "find . -name '*.log' -mtime +7 -exec rm {} \\;",
    "docker run --rm -it -v $(pwd):/src ubuntu bash",
    "kubectl delete namespace staging",
    "chmod -R 777 public",
    "dd if=ubuntu.iso of=/dev/sdb bs=4M status=progress",
    "tar -xzf release.tar.gz -C /opt/app",
    "npm install --save-dev typescript",
    "pip install -r requirements.txt",
    "systemctl restart nginx",
    "ssh deploy@prod.example.com 'sudo reboot'",
    "cat ~/.ssh/id_rsa | nc 10.0.0.5 4444",
    "export PATH=$HOME/.local/bin:$PATH",
    "for f in *.png; do convert \"$f\" \"${f%.png}.jpg\"; done",
    "git clean -fdx",
    "mv config.yaml config.yaml.bak && cp new.yaml config.yaml",
    "frobnicate --all --really",
];

struct Row {
    name: String,
    value: f64,
    unit: &'static str,
    budget: Option<f64>,
    note: String,
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() as f64 - 1.0) * p).round() as usize]
}

struct Env {
    dir: PathBuf,
    bin: PathBuf,
}

impl Env {
    fn new() -> Env {
        let dir = std::env::temp_dir().join(format!("beforerunit-bench-{}", std::process::id()));
        let _ = std::fs::create_dir_all(dir.join("cfg"));
        Env { dir, bin: crate::init::exe_path() }
    }

    fn apply(&self, c: &mut Command) {
        c.env("BEFORERUNIT_CONFIG_DIR", self.dir.join("cfg"))
            .env("BEFORERUNIT_CACHE_DIR", self.dir.join("cache"))
            .env("BEFORERUNIT_SOCKET", self.dir.join("bench.sock"))
            .env("BEFORERUNIT_BIN", &self.bin)
            .env_remove("NO_COLOR")
            .env("COLORTERM", "truecolor")
            .env("TERM", "xterm-256color");
    }

    fn cmd(&self, prog: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut c = Command::new(prog);
        self.apply(&mut c);
        c
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn have(cmd: &str) -> bool {
    Command::new(cmd).arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok()
}

fn time_cmd(mut make: impl FnMut() -> Command, runs: usize) -> Vec<f64> {
    (0..runs)
        .filter_map(|_| {
            let mut c = make();
            c.stdout(Stdio::null()).stderr(Stdio::null()).stdin(Stdio::null());
            let t = Instant::now();
            c.status().ok().filter(|s| s.success())?;
            Some(t.elapsed().as_secs_f64() * 1000.0)
        })
        .collect()
}

pub fn run(pty: bool, strict: bool, slack: f64) -> i32 {
    let slack = if slack.is_finite() { slack.max(1.0) } else { 1.0 };
    let env = Env::new();
    let mut rows: Vec<Row> = Vec::new();
    println!("{} {}", paint::bold("BeforeRunIt bench"), paint::dim("(temporary config, AI off)"));

    // Tier 1 engine, whole lines.
    let mut s = Vec::new();
    for _ in 0..200 {
        for l in CORPUS {
            let t = Instant::now();
            std::hint::black_box(analyze(std::hint::black_box(l)));
            s.push(t.elapsed().as_secs_f64() * 1e6);
        }
    }
    let p50 = pct(&mut s, 0.5);
    rows.push(Row { name: "tier 1 analysis p99".into(), value: pct(&mut s, 0.99), unit: "us", budget: Some(50.0), note: format!("p50 {p50:.1} us") });

    // A keystroke in-process: incremental engine + view + ANSI (what the bash builtin does).
    let theme = Theme::builtin("midnight").unwrap_or_else(|| Theme::resolve("midnight", &[]));
    let mut eng = Engine::new();
    let mut s = Vec::new();
    for _ in 0..20 {
        for l in CORPUS {
            for i in 1..=l.len() {
                if !l.is_char_boundary(i) {
                    continue;
                }
                let t = Instant::now();
                let a = eng.analyze(&l[..i]);
                if let Some(v) = build_view(a, &AiStatus::Off, DisplayMode::Always) {
                    let o = render::Opts {
                        theme: &theme,
                        icons: Default::default(),
                        depth: ColorDepth::True,
                        frame: 0,
                        motion: false,
                        screen_reader: false,
                        max_cols: 100,
                    };
                    std::hint::black_box(render::to_ansi(&render::render(&v, &o), ColorDepth::True));
                }
                s.push(t.elapsed().as_secs_f64() * 1e6);
            }
        }
    }
    let p50 = pct(&mut s, 0.5);
    rows.push(Row { name: "keystroke in-process p99".into(), value: pct(&mut s, 0.99), unit: "us", budget: Some(1000.0), note: format!("p50 {p50:.1} us, engine + render") });

    // Make sure the temp cache exists so child processes start warm.
    let _ = env.cmd(&env.bin).args(["compile", "-q"]).status();

    // bash loadable builtin.
    if let (Some(lib), true) = (crate::init::find_builtin(), have("bash")) {
        let script = format!(
            "enable -f '{}' beforerunit_query || exit 1\n\
             beforerunit_query init '{}'\n\
             l='git push --force origin main'\n\
             s=$EPOCHREALTIME\n\
             for ((i=0;i<2000;i++)); do beforerunit_query draw \"${{l:0:$((i%28+1))}}\" 0 100 2 b '' /tmp 1000 '' $$ ti; done\n\
             e=$EPOCHREALTIME\n\
             echo $(( (${{e/./}} - ${{s/./}}) / 2000 ))",
            lib.display(),
            env.bin.display()
        );
        if let Ok(o) = env.cmd("bash").args(["--norc", "-c", &script]).output() {
            if let Ok(us) = String::from_utf8_lossy(&o.stdout).trim().parse::<f64>() {
                rows.push(Row { name: "bash builtin per keystroke".into(), value: us, unit: "us", budget: Some(1000.0), note: "includes bash function call".into() });
            }
        }
    }

    // One process per keystroke (fish, bash without the builtin).
    let mut s = time_cmd(
        || {
            let mut c = env.cmd(&env.bin);
            c.args(["query", "--shell", "fish", "--cols", "100", "--caps", "ti", "--", "rm -rf build"]);
            c
        },
        60,
    );
    let p50 = pct(&mut s, 0.5);
    rows.push(Row { name: "query process (fish) p99".into(), value: pct(&mut s, 0.99), unit: "ms", budget: Some(3.0), note: format!("p50 {p50:.2} ms") });

    // Daemon: zsh socket round trip, memory, idle CPU.
    let daemon = env.cmd(&env.bin).args(["daemon", "--foreground"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    if let Ok(mut child) = daemon {
        let sock = env.dir.join("bench.sock");
        let mut conn = None;
        for _ in 0..60 {
            std::thread::sleep(Duration::from_millis(25));
            if let Ok(c) = UnixStream::connect(&sock) {
                conn = Some(c);
                break;
            }
        }
        if let Some(c) = conn {
            let _ = c.set_read_timeout(Some(Duration::from_secs(1)));
            let mut w = c.try_clone().expect("socket clone");
            let mut r = BufReader::new(c);
            let mut s = Vec::new();
            let mut line = String::new();
            for i in 0..600 {
                let l = CORPUS[i % CORPUS.len()];
                let l = &l[..(i % l.len()).max(1)];
                let t = Instant::now();
                let _ = writeln!(w, "Q\tbench\t{i}\t100\tti\t\t/tmp\t1000\t{}", beforerunit_core::proto::escape(l));
                line.clear();
                if r.read_line(&mut line).is_err() {
                    break;
                }
                s.push(t.elapsed().as_secs_f64() * 1000.0);
            }
            let p50 = pct(&mut s, 0.5);
            rows.push(Row { name: "zsh socket round trip p99".into(), value: pct(&mut s, 0.99), unit: "ms", budget: Some(1.0), note: format!("p50 {:.0} us", p50 * 1000.0) });
            let pid = child.id();
            let rss = std::fs::read_to_string(format!("/proc/{pid}/status"))
                .ok()
                .and_then(|s| s.lines().find(|l| l.starts_with("VmRSS:")).and_then(|l| l.split_whitespace().nth(1)?.parse::<f64>().ok()))
                .unwrap_or(0.0);
            rows.push(Row { name: "daemon memory (RSS)".into(), value: rss / 1024.0, unit: "MB", budget: Some(15.0), note: String::new() });
            let cpu = |p: u32| -> u64 {
                std::fs::read_to_string(format!("/proc/{p}/stat"))
                    .ok()
                    .and_then(|s| {
                        let rest = &s[s.rfind(')')? + 1..];
                        let f: Vec<&str> = rest.split_whitespace().collect();
                        Some(f.get(11)?.parse::<u64>().ok()? + f.get(12)?.parse::<u64>().ok()?)
                    })
                    .unwrap_or(0)
            };
            std::thread::sleep(Duration::from_millis(500));
            let a = cpu(pid);
            std::thread::sleep(Duration::from_secs(2));
            let b = cpu(pid);
            rows.push(Row { name: "daemon idle CPU".into(), value: (b - a) as f64 / 2.0, unit: "%", budget: Some(0.5), note: format!("{} ticks in 2 s", b - a) });
            let _ = writeln!(w, "STOP");
        }
        let _ = child.wait();
    }

    // Shell startup cost of the integration.
    if have("bash") {
        let base = time_cmd(
            || {
                let mut c = env.cmd("bash");
                c.args(["--norc", "-i", "-c", "exit"]);
                c
            },
            15,
        );
        let with = time_cmd(
            || {
                let mut c = env.cmd("bash");
                c.args(["--norc", "-i", "-c", &format!("eval \"$('{}' init bash)\"; exit", env.bin.display())]);
                c
            },
            15,
        );
        let (mut base, mut with) = (base, with);
        let d = (pct(&mut with, 0.5) - pct(&mut base, 0.5)).max(0.0);
        rows.push(Row { name: "bash init cost".into(), value: d, unit: "ms", budget: Some(5.0), note: "eval \"$(beforerunit init bash)\"".into() });
    }

    // Binary size.
    if let Ok(m) = std::fs::metadata(&env.bin) {
        rows.push(Row { name: "binary size".into(), value: m.len() as f64 / 1_048_576.0, unit: "MB", budget: Some(8.0), note: String::new() });
    }

    if pty {
        for (name, ok) in [("bash", have("bash")), ("fish", have("fish")), ("zsh", have("zsh"))] {
            if !ok {
                continue;
            }
            match pty_keystrokes(&env, name) {
                Some(mut s) if !s.is_empty() => {
                    let p50 = pct(&mut s, 0.5);
                    let budget = if name == "fish" { 12.0 } else { 6.0 };
                    rows.push(Row {
                        name: format!("{name} keystroke to paint p99"),
                        value: pct(&mut s, 0.99),
                        unit: "ms",
                        budget: Some(budget),
                        note: format!("p50 {p50:.2} ms, real terminal round trip"),
                    });
                }
                _ => rows.push(Row { name: format!("{name} keystroke (pty)"), value: 0.0, unit: "", budget: None, note: "could not measure".into() }),
            }
        }
    }

    let mut failed = 0;
    println!();
    for r in &mut rows {
        if matches!(r.unit, "us" | "ms" | "%") {
            r.budget = r.budget.map(|b| b * slack);
        }
    }
    for r in &rows {
        let ok = r.budget.is_none_or(|b| r.value <= b);
        if !ok {
            failed += 1;
        }
        let mark = if r.budget.is_none() {
            paint::dim("\u{f02fd}")
        } else if ok {
            paint::fg(beforerunit_core::render::Rgb(52, 211, 153), "\u{f05e0}")
        } else {
            paint::fg(beforerunit_core::render::Rgb(244, 63, 94), "\u{f0159}")
        };
        let budget = r.budget.map(|b| format!("budget {b} {}", r.unit)).unwrap_or_default();
        println!("  {mark} {:<28} {:>9.2} {:<3} {:<16} {}", r.name, r.value, r.unit, paint::dim(&budget), paint::dim(&r.note));
    }
    println!();
    if failed > 0 {
        println!("{failed} budget(s) missed.");
    } else {
        println!("All budgets met.");
    }
    i32::from(strict && failed > 0)
}

/// Types into a real shell in a pseudo-terminal and times each keystroke
/// until the ghost line is painted.
fn pty_keystrokes(env: &Env, shell: &str) -> Option<Vec<f64>> {
    let dir = &env.dir;
    let bin = env.bin.display().to_string();
    let (argv, marker): (Vec<String>, &[u8]) = match shell {
        "bash" => {
            let rc = dir.join("rc.bash");
            std::fs::write(&rc, format!("PS1='$ '\neval \"$('{bin}' init bash)\"\n")).ok()?;
            (vec!["bash".into(), "--rcfile".into(), rc.display().to_string(), "-i".into()], b"\x1b8")
        }
        "fish" => {
            let cfgd = dir.join("fishcfg/fish");
            std::fs::create_dir_all(&cfgd).ok()?;
            std::fs::write(cfgd.join("config.fish"), format!("function fish_prompt; echo -n '$ '; end\nfunction fish_greeting; end\n'{bin}' init fish | source\n")).ok()?;
            (vec!["fish".into(), "-i".into()], b"\x1b8")
        }
        "zsh" => {
            let zd = dir.join("zdot");
            std::fs::create_dir_all(&zd).ok()?;
            std::fs::write(zd.join(".zshrc"), format!("PS1='$ '\neval \"$('{bin}' init zsh)\"\n")).ok()?;
            (vec!["zsh".into(), "-i".into()], "\u{2570}".as_bytes())
        }
        _ => return None,
    };
    let mut cmd = env.cmd(&argv[0]);
    cmd.args(&argv[1..])
        .env("XDG_CONFIG_HOME", dir.join("fishcfg"))
        .env("XDG_DATA_HOME", dir.join("fishdata"))
        .env("ZDOTDIR", dir.join("zdot"))
        .env("LANG", "C.UTF-8");
    let mut p = Pty::spawn(cmd)?;
    p.drain(Duration::from_millis(1500));
    let mut out = Vec::new();
    for _round in 0..3 {
        for ch in "git push --force origin main".bytes() {
            let t = Instant::now();
            p.write(&[ch]);
            if p.wait_for(marker, Duration::from_millis(500)) {
                out.push(t.elapsed().as_secs_f64() * 1000.0);
            }
            p.drain(Duration::from_millis(15));
        }
        p.write(b"\x15");
        p.drain(Duration::from_millis(200));
    }
    Some(out)
}

struct Pty {
    fd: libc::c_int,
    pid: libc::pid_t,
}

impl Pty {
    fn spawn(cmd: Command) -> Option<Pty> {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let prog = CString::new(cmd.get_program().as_bytes()).ok()?;
        let mut args: Vec<CString> = vec![prog.clone()];
        args.extend(cmd.get_args().filter_map(|a| CString::new(a.as_bytes()).ok()));
        let mut envs: std::collections::BTreeMap<std::ffi::OsString, std::ffi::OsString> = std::env::vars_os().collect();
        for (k, v) in cmd.get_envs() {
            match v {
                Some(v) => {
                    envs.insert(k.to_owned(), v.to_owned());
                }
                None => {
                    envs.remove(k);
                }
            }
        }
        let envp: Vec<CString> = envs
            .iter()
            .filter_map(|(k, v)| {
                let mut s = k.as_bytes().to_vec();
                s.push(b'=');
                s.extend_from_slice(v.as_bytes());
                CString::new(s).ok()
            })
            .collect();
        let mut argp: Vec<*const libc::c_char> = args.iter().map(|a| a.as_ptr()).collect();
        argp.push(std::ptr::null());
        let mut envpp: Vec<*const libc::c_char> = envp.iter().map(|e| e.as_ptr()).collect();
        envpp.push(std::ptr::null());
        let ws = libc::winsize { ws_row: 24, ws_col: 100, ws_xpixel: 0, ws_ypixel: 0 };
        let mut fd = 0;
        // SAFETY: forkpty creates the child; the child only calls execvpe/_exit
        // with pointers prepared before the fork.
        let pid = unsafe { libc::forkpty(&mut fd, std::ptr::null_mut(), std::ptr::null(), &ws) };
        if pid < 0 {
            return None;
        }
        if pid == 0 {
            unsafe {
                libc::execvpe(prog.as_ptr(), argp.as_ptr(), envpp.as_ptr());
                libc::_exit(127);
            }
        }
        Some(Pty { fd, pid })
    }

    fn write(&mut self, b: &[u8]) {
        // SAFETY: writing a valid buffer to our pty master.
        unsafe {
            libc::write(self.fd, b.as_ptr() as *const _, b.len());
        }
    }

    fn read_some(&mut self, timeout: Duration, buf: &mut Vec<u8>) -> bool {
        let mut pfd = libc::pollfd { fd: self.fd, events: libc::POLLIN, revents: 0 };
        // SAFETY: poll/read on our own fd with a stack buffer.
        unsafe {
            if libc::poll(&mut pfd, 1, timeout.as_millis() as i32) <= 0 {
                return false;
            }
            let mut tmp = [0u8; 8192];
            let n = libc::read(self.fd, tmp.as_mut_ptr() as *mut _, tmp.len());
            if n <= 0 {
                return false;
            }
            let data = &tmp[..n as usize];
            // fish asks for the primary device attributes and waits for them.
            if data.windows(4).any(|w| w == b"\x1b[0c") || data.windows(3).any(|w| w == b"\x1b[c") {
                libc::write(self.fd, b"\x1b[?62;22c".as_ptr() as *const _, 9);
            }
            buf.extend_from_slice(data);
        }
        true
    }

    fn drain(&mut self, d: Duration) {
        let end = Instant::now() + d;
        let mut sink = Vec::new();
        while Instant::now() < end {
            let left = end.saturating_duration_since(Instant::now());
            if !self.read_some(left.min(Duration::from_millis(20)), &mut sink) {
                continue;
            }
            sink.clear();
        }
    }

    fn wait_for(&mut self, marker: &[u8], timeout: Duration) -> bool {
        let end = Instant::now() + timeout;
        let mut buf = Vec::new();
        while Instant::now() < end {
            let left = end.saturating_duration_since(Instant::now());
            self.read_some(left.min(Duration::from_millis(50)), &mut buf);
            if buf.windows(marker.len()).any(|w| w == marker) {
                return true;
            }
        }
        false
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        // SAFETY: signals and closes only the child and fd we created.
        unsafe {
            libc::kill(self.pid, libc::SIGKILL);
            libc::waitpid(self.pid, std::ptr::null_mut(), 0);
            libc::close(self.fd);
        }
    }
}