# BeforeRunIt

**Know what a command does before you press Enter.**

BeforeRunIt draws a live line under your shell prompt while you type. It shows
a risk badge and a plain-English summary of at most ten words, and it updates
on every keystroke and paste.

```text
$ rm -rf build
╰─  DANGER  Deletes "build" forever. Skips Trash.

$ git push --force-with-lease origin main
╰─  CAUTION  Force-pushes "main", only if nobody else pushed.

$ tar tzf release.tar.gz
╰─  KNOWN  Lists what is inside "release.tar.gz".

$ frobnicate --all
╰─  UNKNOWN  Not in the rule book. Check before running.
```

| Badge | Meaning |
|---|---|
| KNOWN | In the rule book and safe in this form |
| CAUTION | Changes things, but can usually be undone |
| DANGER | Destroys data, rewrites history, or opens up permissions |
| UNKNOWN | Not in the rule book; the optional AI review can explain it |

Works with **bash, zsh and fish** on any Linux distro. Written in Rust.
Licensed under MIT or Apache-2.0.

## Install

```sh
curl -fsSL https://github.com/beforerunit/beforerunit/releases/latest/download/install.sh | sh
```

The installer puts `beforerunit` (and the short alias `bri`) in `~/.local/bin`,
or `/usr/local/bin` when run as root. It installs the bash fast path in
`lib/beforerunit/` next to it and adds one line to the startup file of your
login shell. Run `sh install.sh --help` for options such as `--prefix`,
`--shell` and `--no-modify-rc`.

Other ways to install:

- **Debian, Ubuntu:** download the `.deb` from the
  [releases](https://github.com/beforerunit/beforerunit/releases), then
  `sudo apt install ./beforerunit_*.deb`.
- **Fedora, openSUSE:** download the `.rpm`, then `sudo dnf install ./beforerunit-*.rpm`.
- **Arch:** build with [`packaging/aur/PKGBUILD`](packaging/aur/PKGBUILD).
- **Nix:** `nix profile install github:beforerunit/beforerunit`
- **From source:**
  ```sh
  cargo build --release
  cargo build --profile release-bash -p beforerunit-bash   # bash fast path
  ```

### Hook up your shell

The installer does this for you. To do it by hand, add one line:

| Shell | File | Line |
|---|---|---|
| bash | `~/.bashrc` | `eval "$(beforerunit init bash)"` |
| zsh | `~/.zshrc` | `eval "$(beforerunit init zsh)"` |
| fish | `~/.config/fish/config.fish` | `beforerunit init fish \| source` |

With [ble.sh](https://github.com/akinomyoga/ble.sh), place the bash line
before `ble-attach`; BeforeRunIt then draws through ble.sh's own info area.

Then open a new terminal and run `beforerunit doctor` to check everything.

### Fonts

The badges use [Nerd Font](https://www.nerdfonts.com) icons. If your terminal
shows boxes instead of icons, install a font:

```sh
beforerunit fonts install            # Maple Mono NF (default)
beforerunit fonts install jetbrains  # JetBrainsMono Nerd Font
```

Fonts go into `~/.local/share/fonts/beforerunit/`. Then select the font in
your terminal settings; `beforerunit fonts status` tells you where that
setting lives for common terminals. If you would rather not change fonts,
pick `icons = "unicode"` or `"ascii"` in `beforerunit setup`.

## Usage

Just type. The line follows along, and Enter clears it before the command runs.

| Command | What it does |
|---|---|
| `beforerunit setup` | Settings app: AI provider, theme, display, rule browser |
| `beforerunit explain CMD...` | Full breakdown of one command (exit code 0 known, 1 unknown, 2 caution, 3 danger) |
| `beforerunit doctor` | Checks shells, terminal, fonts, config and AI |
| `beforerunit test-ai` | Sends a sample command to your AI endpoint |
| `beforerunit fonts install` | Installs a Nerd Font |
| `beforerunit bench` | Measures speed against the budgets below |
| `beforerunit uninstall` | Removes hooks, cache, settings and fonts |

**Alt-B** turns the line off and on for the current shell (change it with
`toggle_key`).

## AI review (optional)

The rule book covers about 160 common commands and needs no network. For
everything else, and as a second opinion on risky commands, BeforeRunIt can
ask any **OpenAI-compatible** endpoint: Ollama, LM Studio, Groq, OpenRouter,
OpenAI, or your own server.

```sh
beforerunit setup     # pick a provider, paste a key, fetch models, test
beforerunit test-ai   # or test from the command line
```

How it works:

- Tier 1, the local rule engine, answers in microseconds on every keystroke.
- Tier 2, the AI, runs in a background daemon 300 ms after you stop typing.
  The answer appears in place with an AI tag, and results are cached for 7 days.
- `ai.mode`:
  - `unknown-only` asks only about commands the rule book does not know;
  - `review-all` also reviews known commands;
  - `off` disables the AI.
- The AI may **raise** a risk at any time. It may **lower** one by a single
  level only when its confidence is at least 0.8, and never below a hard rule.
  For example, `rm -rf /` is always DANGER.

**Privacy.** The AI sees only:
- the command line, with secrets such as tokens, passwords and keys masked;
- the current folder's name;
- the git state (branch, dirty or clean);
- whether you are root;
- whether the files the command targets exist.

It never sees file contents, your environment or your history.

**API keys** are stored in the system keyring (Secret Service). Without a
keyring, they go in a file readable only by you (mode 0600). The
`BEFORERUNIT_API_KEY` environment variable overrides both.

## Configuration

`~/.config/beforerunit/config.toml`. The setup app writes it for you; these
are the defaults:

```toml
[ai]
mode = "unknown-only"          # off | unknown-only | review-all
provider = "ollama"
base_url = "http://localhost:11434/v1"
model = "qwen2.5:0.5b"
timeout_ms = 4000
debounce_ms = 300
min_confidence_to_lower = 0.8

[ui]
display = "always"             # always | risky-only | off
theme = "midnight"             # midnight | nord | catppuccin | gruvbox | contrast | mono
icons = "nerd"                 # nerd | unicode | ascii
reduce_motion = false          # no light sweep or glow
screen_reader = false          # plain "[DANGER] ..." text, no animation
toggle_key = "\\eb"            # Alt-B
```

`NO_COLOR` is respected. With `screen_reader = true` the line becomes plain
text that screen readers announce cleanly.

### Your own rules

Drop TOML files into `~/.config/beforerunit/rules.d/`. They are compiled on
the next shell start, or right away with `beforerunit compile`. The format is
the same as the built-in [`rules/`](rules):

```toml
[[cmd]]
name = "deploy"
risk = "caution"
text = "Deploys {target} to production."

  [[cmd.rule]]
  flags_any = ["force", "f"]
  risk = "danger"
  text = "Force-deploys, skipping checks."
```

## Speed

Speed is a feature: the line must never make typing feel slower.
`beforerunit bench --pty` measures each of these on your machine, and CI fails
when a budget is missed.

| Path | Budget | Typical |
|---|---|---|
| Rule engine per keystroke | 50 µs | 3 µs |
| bash keystroke to paint (loadable builtin, no process spawned) | 1 ms in-process, 6 ms through the terminal | 12 µs, 0.9 ms |
| zsh keystroke (persistent socket to the daemon) | 1 ms round trip | 0.05 ms |
| fish keystroke (one small process) | 3 ms | 2 ms |
| Shell startup cost | 5 ms | 3.8 ms |
| Daemon memory, idle CPU | 15 MB, 0% | 9 MB, 0% |
| Binary size | 8 MB | 7.2 MB |

## How it works

- **bash.** A native loadable builtin (`libbeforerunit_bash.so`) runs the
  engine inside bash itself, so no process is started per keystroke. Without
  it, a fallback starts one small process per keystroke.
- **zsh.** Draws through `POSTDISPLAY` and region highlighting, so it works
  alongside zsh-autosuggestions. It talks to the daemon over a persistent
  socket.
- **fish.** Uses key bindings with `commandline` and runs one small process
  per keystroke.
- **Daemon.** Started on demand and stopped after 3 hours idle. It debounces
  and cancels AI requests per session, caches answers in memory and on disk,
  and delivers results without disturbing what you type.

The crates:
- `beforerunit-core`: lexer, rules, rendering;
- `beforerunit-ai`: OpenAI-compatible client;
- `beforerunit-bash`: the builtin;
- `beforerunit`: the command-line tool, daemon and settings app.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Rule additions are the easiest place
to start. Security issues: [SECURITY.md](SECURITY.md).

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at
your option.
