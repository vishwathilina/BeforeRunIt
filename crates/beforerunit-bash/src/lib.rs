//! Bash loadable builtin: `enable -f libbeforerunit_bash.so beforerunit_query`.
//!
//! Usage from the bash plugin:
//!   beforerunit_query draw  LINE POINT COLS PROMPT_WIDTH SID MODE CWD EUID TTY PID CAPS
//!   beforerunit_query line  (same as draw) only the styled line, for ble.sh
//!   beforerunit_query clear LINE POINT COLS PROMPT_WIDTH SID
//!   beforerunit_query forget
//! The escape sequence to print is stored in `$BRI_OUT`.
//!
//! Everything runs inside the bash process, so panics are caught at the FFI
//! boundary (build with `--profile release-bash`, which unwinds).

use std::ffi::{c_char, c_int, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;
use std::sync::Mutex;

use beforerunit_core::painter::{binary_path, DrawArgs, Painter};
use beforerunit_core::store::Compiled;

#[repr(C)]
pub struct WordDesc {
    word: *const c_char,
    flags: c_int,
}

#[repr(C)]
pub struct WordList {
    next: *const WordList,
    word: *const WordDesc,
}

type BuiltinFn = extern "C" fn(*const WordList) -> c_int;

/// Mirrors `struct builtin` from bash's `builtins.h`.
#[repr(C)]
pub struct Builtin {
    name: *const c_char,
    function: BuiltinFn,
    flags: c_int,
    long_doc: *const *const c_char,
    short_doc: *const c_char,
    handle: *const c_char,
}

// SAFETY: the struct only holds pointers to static, immutable data.
unsafe impl Sync for Builtin {}

const BUILTIN_ENABLED: c_int = 0x01;
const EXECUTION_SUCCESS: c_int = 0;
const EXECUTION_FAILURE: c_int = 1;

struct Doc([*const c_char; 2]);
// SAFETY: points at static string literals.
unsafe impl Sync for Doc {}

static LONG_DOC: Doc = Doc([c"Draws the BeforeRunIt ghost line. Used by the bash plugin.".as_ptr(), ptr::null()]);

// Must be writable: bash's `enable -f` stores the dlopen handle and adjusts
// `flags` inside this struct after loading it.
#[no_mangle]
pub static mut beforerunit_query_struct: Builtin = Builtin {
    name: c"beforerunit_query".as_ptr(),
    function: beforerunit_query,
    flags: BUILTIN_ENABLED,
    long_doc: &LONG_DOC.0 as *const _ as *const *const c_char,
    short_doc: c"beforerunit_query draw|clear|forget ...".as_ptr(),
    handle: ptr::null(),
};

extern "C" {
    // Exported by the bash executable; resolved when the builtin is loaded.
    fn bind_variable(name: *const c_char, value: *const c_char, flags: c_int) -> *mut libc::c_void;
}

static PAINTER: Mutex<Option<Painter>> = Mutex::new(None);

fn recompile() -> Option<Compiled> {
    // Rare (config edited): let the CLI parse TOML so bash never links a TOML parser.
    let ok = std::process::Command::new(binary_path())
        .arg("compile")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if ok {
        beforerunit_core::store::load()
    } else {
        None
    }
}

fn set_out(s: &str) {
    let clean: Vec<u8> = s.bytes().filter(|b| *b != 0).collect();
    let Ok(v) = CString::new(clean) else { return };
    // SAFETY: both pointers are valid NUL-terminated strings for the call;
    // bash copies the value.
    unsafe {
        bind_variable(c"BRI_OUT".as_ptr(), v.as_ptr(), 0);
    }
}

fn args(list: *const WordList) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = list;
    // SAFETY: bash passes a valid, NULL-terminated linked list of words.
    unsafe {
        while !cur.is_null() {
            let w = (*cur).word;
            if !w.is_null() && !(*w).word.is_null() {
                out.push(CStr::from_ptr((*w).word).to_string_lossy().into_owned());
            }
            cur = (*cur).next;
        }
    }
    out
}

fn run(argv: &[String]) -> String {
    let get = |i: usize| argv.get(i).map(String::as_str).unwrap_or("");
    let num = |i: usize| get(i).parse::<usize>().unwrap_or(0);
    if get(0) == "init" {
        beforerunit_core::painter::set_binary_path(get(1));
        return String::new();
    }
    let mut guard = match PAINTER.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    let painter = guard.get_or_insert_with(|| Painter::new(Some(recompile)));
    let mut a = DrawArgs {
        line: get(1),
        point: num(2),
        cols: num(3).max(20),
        prompt_width: num(4),
        sid: get(5),
        mode: "",
        cwd: "",
        euid: "",
        tty: "",
        pid: "",
        from_line_start: true,
        caps: "",
    };
    let cmd = get(0);
    if cmd == "draw" || cmd == "line" {
        a.mode = get(6);
        a.cwd = get(7);
        a.euid = get(8);
        a.tty = get(9);
        a.pid = get(10);
        a.caps = get(11);
    }
    match cmd {
        "draw" => painter.draw(&a),
        "line" => {
            let (_, down) = beforerunit_core::painter::rows_below(a.prompt_width, a.line, a.point, a.cols);
            painter.line(&a, down)
        }
        "clear" => painter.clear(&a),
        "forget" => {
            if painter.forget() {
                "\x1b[J".into()
            } else {
                String::new()
            }
        }
        _ => String::new(),
    }
}

extern "C" fn beforerunit_query(list: *const WordList) -> c_int {
    let argv = args(list);
    match catch_unwind(AssertUnwindSafe(|| run(&argv))) {
        Ok(out) => {
            set_out(&out);
            EXECUTION_SUCCESS
        }
        Err(_) => {
            set_out("");
            EXECUTION_FAILURE
        }
    }
}
