#![no_main]

use beforerunit_core::{analyze, lexer, Engine};
use libfuzzer_sys::fuzz_target;

// The lexer and engine run on every keystroke inside the user's shell, so they
// must never panic on any input, including every prefix of it.
fuzz_target!(|data: &[u8]| {
    let Ok(s) = std::str::from_utf8(data) else { return };
    let _ = lexer::lex(s);
    let _ = analyze(s);
    let mut e = Engine::new();
    for (i, _) in s.char_indices() {
        let _ = e.analyze(&s[..i]);
    }
});
