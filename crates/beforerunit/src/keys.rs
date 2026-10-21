//! API key storage. Lookup order: `BEFORERUNIT_API_KEY`, the system keyring
//! (Secret Service), then `~/.config/beforerunit/api_key` (mode 0600).

use std::fs;
use std::io::{IsTerminal, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;

const SERVICE: &str = "beforerunit";
const USER: &str = "api_key";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Where {
    Env,
    Keyring,
    File,
}

impl Where {
    pub fn describe(self) -> String {
        match self {
            Where::Env => "BEFORERUNIT_API_KEY environment variable".into(),
            Where::Keyring => "system keyring (Secret Service)".into(),
            Where::File => format!("file {} (mode 0600)", key_file().display()),
        }
    }
}

fn key_file() -> PathBuf {
    beforerunit_core::store::config_dir().join("api_key")
}

/// keyring talks to D-Bus through its own runtime; run it on a plain thread
/// so it never nests inside the daemon's tokio runtime.
fn on_thread<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    std::thread::spawn(f).join().ok()
}

fn keyring_get() -> Option<String> {
    on_thread(|| keyring::Entry::new(SERVICE, USER).ok()?.get_password().ok()).flatten().filter(|k| !k.is_empty())
}

fn keyring_set(key: String) -> bool {
    on_thread(move || keyring::Entry::new(SERVICE, USER).and_then(|e| e.set_password(&key)).is_ok()).unwrap_or(false)
}

fn keyring_delete() -> bool {
    on_thread(|| keyring::Entry::new(SERVICE, USER).and_then(|e| e.delete_credential()).is_ok()).unwrap_or(false)
}

fn file_get() -> Option<String> {
    let k = fs::read_to_string(key_file()).ok()?;
    let k = k.trim().to_string();
    (!k.is_empty()).then_some(k)
}

fn file_set(key: &str) -> std::io::Result<()> {
    let p = key_file();
    if let Some(d) = p.parent() {
        fs::create_dir_all(d)?;
    }
    let mut f = fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&p)?;
    f.write_all(key.as_bytes())?;
    f.write_all(b"\n")
}

pub fn get() -> Option<(String, Where)> {
    if let Ok(k) = std::env::var("BEFORERUNIT_API_KEY") {
        if !k.is_empty() {
            return Some((k, Where::Env));
        }
    }
    if let Some(k) = keyring_get() {
        return Some((k, Where::Keyring));
    }
    file_get().map(|k| (k, Where::File))
}

pub fn api_key() -> Option<String> {
    get().map(|(k, _)| k)
}

/// Stores the key, preferring the keyring. Returns where it went.
pub fn set(key: &str) -> std::io::Result<Where> {
    let key = key.trim();
    if keyring_set(key.to_string()) && keyring_get().as_deref() == Some(key) {
        let _ = fs::remove_file(key_file());
        return Ok(Where::Keyring);
    }
    file_set(key)?;
    Ok(Where::File)
}

pub fn delete() {
    keyring_delete();
    let _ = fs::remove_file(key_file());
}

pub fn mask(k: &str) -> String {
    let n = k.chars().count();
    if n <= 8 {
        return "*".repeat(n);
    }
    let head: String = k.chars().take(4).collect();
    let tail: String = k.chars().skip(n - 4).collect();
    format!("{head}...{tail}")
}

pub fn cli_set() -> i32 {
    let mut key = String::new();
    if std::io::stdin().is_terminal() {
        eprint!("API key (input hidden): ");
        let _ = std::io::stderr().flush();
        key = read_hidden().unwrap_or_default();
        eprintln!();
    } else {
        let _ = std::io::stdin().read_to_string(&mut key);
    }
    if key.trim().is_empty() {
        eprintln!("beforerunit: no key given");
        return 1;
    }
    match set(&key) {
        Ok(w) => {
            println!("Stored in the {}.", w.describe());
            let _ = beforerunit_core::proto::send("RELOAD");
            0
        }
        Err(e) => {
            eprintln!("beforerunit: cannot store key: {e}");
            1
        }
    }
}

pub fn cli_delete() -> i32 {
    delete();
    let _ = beforerunit_core::proto::send("RELOAD");
    println!("API key removed.");
    0
}

pub fn cli_status() -> i32 {
    match get() {
        Some((k, w)) => println!("{} in the {}", mask(&k), w.describe()),
        None => println!("No API key stored. Local providers like Ollama do not need one."),
    }
    0
}

fn read_hidden() -> Option<String> {
    // SAFETY: plain termios calls on stdin; the original mode is restored.
    unsafe {
        let mut t: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(0, &mut t) != 0 {
            return None;
        }
        let orig = t;
        t.c_lflag &= !libc::ECHO;
        libc::tcsetattr(0, libc::TCSANOW, &t);
        let mut s = String::new();
        let r = std::io::stdin().read_line(&mut s);
        libc::tcsetattr(0, libc::TCSANOW, &orig);
        r.ok()?;
        Some(s.trim().to_string())
    }
}
