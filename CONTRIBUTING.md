# Contributing to BeforeRunIt

Thanks for helping. Rule additions, rule fixes and shell compatibility reports
are the most valuable contributions.

## Build and test

```sh
cargo build --release                                     # binary
cargo build --profile release-bash -p beforerunit-bash    # bash builtin
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

The bash builtin needs its own profile because it runs inside bash: a panic
must unwind into `catch_unwind` instead of aborting the user's shell.

To try your build in a shell without installing:

```sh
eval "$(target/release/beforerunit init bash)"
```

Point `BEFORERUNIT_CONFIG_DIR`, `BEFORERUNIT_CACHE_DIR` and
`BEFORERUNIT_SOCKET` at a scratch directory to keep your real settings
untouched.

## Layout

| Path | What |
|---|---|
| `rules/*.toml` | Built-in rule book, compiled into the binary by `build.rs` |
| `crates/beforerunit-core` | Lexer, rule engine, ghost-line rendering, painter, wire protocol |
| `crates/beforerunit-ai` | OpenAI-compatible client, prompt, redaction, verdict parsing |
| `crates/beforerunit-bash` | bash loadable builtin (`enable -f`) |
| `crates/beforerunit` | CLI, daemon, settings app, doctor, bench |
| `crates/beforerunit/shell/` | bash, zsh and fish integration scripts |
| `fuzz/` | cargo-fuzz targets for the lexer |

## Writing rules

Each `[[cmd]]` has a default verdict and a list of `[[cmd.rule]]` overrides.
The first matching rule wins, so put the most specific rules first.

- `sub`: subcommand, for example `push` for `git push`.
- `flags_all`, `flags_any`: normalized flags. `-rf` becomes `r` and `f`. Long
  aliases are mapped through `long = { force = "f" }`.
- `args_any`: glob patterns on positional arguments.
- `target = "last" | "first" | "all"`: which argument fills `{target}`.
- `hard = true`: the AI can never lower this verdict.
- `text` must be plain English, at most 10 words, no jargon. `pending` is the
  wording shown while the command is still being typed.

Every rule change should come with a case in
`crates/beforerunit-core/tests/progression.rs`, or a snapshot update:

```sh
cargo test -p beforerunit-core
cargo insta review   # if snapshots changed
```

## Performance rules

The line must never make typing feel slower. Hard budgets, checked by
`beforerunit bench --pty --strict` in CI:

- Tier 1 analysis under 50 µs, keystroke handling under 1 ms for bash and zsh.
- Shell startup cost under 5 ms, daemon under 15 MB and 0% CPU when idle.
- Binary under 8 MB.

On the keystroke path, avoid allocations in the lexer and engine (they use
`SmallVec` and borrowed slices), never block on I/O, and never start a process
in bash when the builtin is loaded. Criterion benches:
`cargo bench -p beforerunit-core`.

### Profile-guided optimization

PGO gives another 10 to 20 percent on the engine. It is optional, and release
builds do not use it yet:

```sh
cargo install cargo-pgo
cargo pgo build
target/x86_64-unknown-linux-gnu/release/beforerunit bench   # training run
cargo pgo optimize
```

## Shell scripts

- Keep bash compatible with 4.4+, zsh with 5.3+, and fish with 3.5+.
- Startup work must stay tiny. Bash key bindings are loaded from a generated
  inputrc with one `bind -f`, because separate `bind` calls cost about 15 µs each.
- Test interactively. CI also runs `bash -n`, `zsh -n` and
  `fish --no-execute` on the generated scripts.

## Demo GIF

`docs/demo.tape` renders with [vhs](https://github.com/charmbracelet/vhs):
`vhs docs/demo.tape`, or run the "Demo GIF" workflow on GitHub.

## Pull requests

- One topic per pull request, with tests.
- No emoji in code, output or docs. Use Nerd Font icons with Unicode and
  ASCII fallbacks.
- By contributing you agree your work is dual-licensed under MIT or
  Apache-2.0.
