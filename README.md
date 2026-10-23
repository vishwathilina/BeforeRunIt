<p align="center">
  <img src="docs/logo.png" alt="BeforeRunIt" width="520">
</p>

<p align="center">
  <b>Your finger is on Enter. Read the line under it first.</b>
</p>

<p align="center">
  BeforeRunIt watches the command you are typing and says, in plain English,<br>
  what it will actually do. One line. About ten words. On every keystroke.<br>
  bash, zsh and fish. On your machine. No account, no cloud.
</p>

## The half-second that matters

The dangerous command and the safe one are often the same letters.

```text
$ rm -rf build/
╰─  DANGER  Deletes "build/" forever. Skips Trash.

$ rm -rf build /
╰─  DANGER  Deletes your entire system. Unrecoverable.
```

One space. That is the difference between a folder and your whole machine. The man page explains it too, but not in the moment your finger is already moving. BeforeRunIt does.

Same idea, other shapes:

```text
$ curl https://example.com/install.sh | sh
╰─  DANGER  Runs an internet script without checking it.

$ git push --force origin main
╰─  DANGER  Overwrites remote history of "main". Others lose work.

$ git status
╰─  KNOWN  Runs git status.

$ ncdu
╰─  UNKNOWN  Not in the rule book. Check before running.
```

`git status` only looks. `ncdu` is a real, useful program that the rule book has simply not met yet, so the line says so instead of pretending.

## The four words

Every command gets one of these. That is the whole idea.

<table>
<tr>
<td align="center" width="25%">
<img src="docs/bear-known.png" alt="A happy bear giving a thumbs up" width="150"><br>
<b>KNOWN</b><br>
Safe. This one only looks, or it is harmless.
</td>
<td align="center" width="25%">
<img src="docs/bear-caution.png" alt="A bear thinking, with a light bulb" width="150"><br>
<b>CAUTION</b><br>
Slow down. This changes something, but you can usually undo it.
</td>
<td align="center" width="25%">
<img src="docs/bear-danger.png" alt="A worried bear covering its eyes" width="150"><br>
<b>DANGER</b><br>
Stop. This can delete files, break a system, or throw work away.
</td>
<td align="center" width="25%">
<img src="docs/bear-unknown.png" alt="A confused bear with question marks" width="150"><br>
<b>UNKNOWN</b><br>
Not in the rule book yet. Read it yourself before you run it.
</td>
</tr>
</table>

## What that looks like

<table>
<tr>
<td align="center" width="50%">
<img src="docs/scene-danger.jpg" alt="The bear panics as the terminal shows a DANGER badge on rm -rf build"><br>
<b>DANGER.</b> The line spoke before Enter did.
</td>
<td align="center" width="50%">
<img src="docs/scene-known.jpg" alt="The bear gives a thumbs up as the terminal shows a KNOWN badge"><br>
<b>KNOWN.</b> All good. Carry on.
</td>
</tr>
<tr>
<td align="center">
<img src="docs/scene-unknown.jpg" alt="The bear is confused by an UNKNOWN badge"><br>
<b>UNKNOWN.</b> Never seen this command. Check it first.
</td>
<td align="center">
<img src="docs/scene-setup.jpg" alt="The bear has an idea while running beforerunit setup"><br>
<b>Want a second opinion?</b> That is what setup is for.
</td>
</tr>
</table>

The pictures are the mascot. The lines above are exactly what your terminal prints, and they update on every key.

## Install

One command. It works for a normal user, with no root password:

```sh
curl -fsSL https://github.com/vishwathilina/BeforeRunIt/releases/latest/download/install.sh | sh
```

Then open a **new** terminal and type `ls`. The line should show up underneath.

Already cloned this repository? Run `./install.sh` from the project folder.
It installs the copy you built (and builds it first, if you have not).

The installer puts three things in place:

- `beforerunit`, the program, in `~/.local/bin` (or `/usr/local/bin` if you are root)
- `bri`, a shorter name for the same program
- one line in the startup file of your login shell, so it loads every time

If `~/.local/bin` is not on your `PATH`, the installer says so. The line under
your prompt still works. Add that folder to your `PATH` when you want to type
`beforerunit` yourself.

### Add the line yourself

Only needed when you installed with `--no-modify-rc`, or the installer could
not see your shell.

| Shell | Add this line to | |
|---|---|---|
| bash | `~/.bashrc` | `eval "$(beforerunit init bash)"` |
| zsh | `~/.zshrc` | `eval "$(beforerunit init zsh)"` |
| fish | `~/.config/fish/config.fish` | `beforerunit init fish \| source` |

Using [ble.sh](https://github.com/akinomyoga/ble.sh)? Put the bash line
**before** `ble-attach`.

Other installs: `.deb` and `.rpm` from the
[releases](https://github.com/vishwathilina/BeforeRunIt/releases),
[`packaging/aur/PKGBUILD`](packaging/aur/PKGBUILD) for Arch, or
`nix profile install github:vishwathilina/BeforeRunIt`.

### Check that it worked

```sh
beforerunit doctor
```

Green marks are fine. A yellow mark comes with the exact command to fix it.

### The icons look like empty boxes

The badges use a [Nerd Font](https://www.nerdfonts.com). If you see boxes
instead of small icons:

```sh
beforerunit fonts install
```

That downloads Maple Mono and tells you where to select it in your terminal.
You can also skip the icons: in `beforerunit setup`, set icons to `unicode`
or `ascii` and the badges become plain text.

## Day to day

Just type. The line follows along. Pressing Enter clears it, then your
command runs as usual.

- **Alt-B** hides the line. Press it again to bring it back.
- `beforerunit explain rm -rf build` prints the full reason, not just the ten words.
- `beforerunit setup` opens the settings screen: colors, when to show the line, and the AI.
- In that screen, **Ctrl-S saves**. Closing it with `q` throws the changes away.

| Command | What it does |
|---|---|
| `beforerunit setup` | Settings: colors, the AI, and the rule book |
| `beforerunit explain CMD` | The full reason for one command |
| `beforerunit doctor` | Checks that the shell, the font and the AI are set up |
| `beforerunit test-ai` | Sends one sample command to your AI, to prove the key works |
| `beforerunit fonts install` | Downloads a font that can draw the icons |
| `beforerunit uninstall` | Removes the shell line, the settings and the font |

`explain` uses its exit code so scripts can read it: 0 known, 1 unknown,
2 caution, 3 danger.

## A second opinion, from an AI you choose

You do not need this. The rule book covers about 160 common commands and it
never touches the network.

For commands it does not know, you can point BeforeRunIt at any server that
speaks the OpenAI API: Ollama or LM Studio on your own machine (free), or
Groq, OpenRouter or OpenAI.

```sh
beforerunit setup
```

Pick a provider, paste your key, press `f` to fetch the model list, press `t`
to test, then **Ctrl-S** to save. Open a new terminal afterwards.

A small instruct model is the right tool here. Reasoning models spend the
whole reply "thinking" and often send back nothing useful.

What the AI is allowed to do:

- It can raise the warning whenever it disagrees.
- It can lower the warning by one step, and only when it is very sure.
- It can never lower a hard rule. `rm -rf /` stays DANGER no matter what it says.

What the AI is allowed to see: the command, with passwords and keys blanked
out, the folder name, whether it is a git repo, whether you are root, and
whether the files you named exist. Never your file contents, your environment,
or your history.

Your key is stored in the system keyring. With no keyring, it goes in a file
only your user can read. `BEFORERUNIT_API_KEY` overrides both.

## Support this project

BeforeRunIt is free, open source, and built in spare time. If it has ever
stopped you pressing Enter on the wrong command, you can help keep it going.

<table>
<tr>
<td width="180">
<img src="docs/bear-coffee.png" alt="The bear holding a mug of coffee" width="160">
</td>
<td>
<a href="https://buymeacoffee.com/vishwathilina"><img src="https://cdn.buymeacoffee.com/buttons/v2/default-yellow.png" alt="Buy Me A Coffee" height="60"></a>
<br><br>
One coffee goes a long way. Thank you.
</td>
</tr>
</table>

## Settings

`beforerunit setup` writes this file for you. It lives at
`~/.config/beforerunit/config.toml`.

```toml
[ai]
mode = "unknown-only"          # off, unknown-only, or review-all
provider = "ollama"
base_url = "http://localhost:11434/v1"
model = "qwen2.5:0.5b"
timeout_ms = 4000
debounce_ms = 300

[ui]
display = "always"             # always, risky-only, or off
theme = "midnight"             # midnight, nord, catppuccin, gruvbox, contrast, mono
icons = "nerd"                 # nerd, unicode, or ascii
reduce_motion = false
screen_reader = false          # plain text, no animation, easy to hear
toggle_key = "\\eb"            # Alt-B
```

`risky-only` hides the line for KNOWN commands, so it only speaks up when
something needs a look. `screen_reader = true` prints `[DANGER] ...` as
ordinary text. `NO_COLOR` is respected.

### Your own rules

Any command can be taught. Drop a file in `~/.config/beforerunit/rules.d/`
and open a new terminal (or run `beforerunit compile`):

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

`text` is what the line will say, in plain words, ten words at most. The
built-in rules in [`rules/`](rules) are full of examples.

## For the curious

The line has to keep up with your typing, so there are hard speed limits.
`beforerunit bench` checks them on your machine.

| What | Must stay under | Usually |
|---|---|---|
| Deciding the risk for one keystroke | 50 µs | 3 µs |
| bash, keystroke until the line is drawn | 6 ms | 0.9 ms |
| zsh, same | 6 ms | 2 ms |
| fish, same | 12 ms | 4 ms |
| Time added when the shell starts | 5 ms | 3.8 ms |
| Memory while idle | 15 MB | 9 MB |
| Program size | 8 MB | 7.2 MB |

bash runs the check inside the shell itself, so it does not start a new
program on every key. zsh keeps one connection open to a small background
helper. fish starts one small program per keystroke. The helper starts on its
own the first time the AI is needed, and it exits after a few idle hours.

The code is split into `beforerunit-core` (the rules and the line),
`beforerunit-ai` (the AI client), `beforerunit-bash` (the in-shell piece)
and `beforerunit` (the program you run).

## Contributing

Rule ideas are the best place to start. See
[CONTRIBUTING.md](CONTRIBUTING.md). Security issues go through
[SECURITY.md](SECURITY.md), not the public issues.

## License

[MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), whichever you prefer.
