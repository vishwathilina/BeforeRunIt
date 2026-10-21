//! BeforeRunIt: a live risk line under your shell prompt.

mod bench;
mod cache;
mod context;
mod daemon;
mod doctor;
mod explain;
mod fonts;
mod init;
mod keys;
mod query;
mod setup;
mod testai;
mod ui;
mod uninstall;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(
    name = "beforerunit",
    version,
    about = "Live command risk line for Linux shells",
    long_about = "BeforeRunIt shows a risk badge (KNOWN, CAUTION, DANGER, UNKNOWN) and a short plain-English \
summary under your prompt while you type, before you press Enter.\n\n\
Quick start:\n  echo 'eval \"$(beforerunit init bash)\"' >> ~/.bashrc   (zsh: ~/.zshrc)\n  echo 'beforerunit init fish | source' >> ~/.config/fish/config.fish\n  beforerunit setup     (optional AI review)\n  beforerunit doctor"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Clone, Copy, ValueEnum, PartialEq, Eq, Debug)]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
}

#[derive(Subcommand)]
enum Cmd {
    /// Print the shell integration: eval "$(beforerunit init bash)"
    Init {
        shell: Shell,
        /// Plain bash fallback even when the fast builtin is available
        #[arg(long)]
        no_builtin: bool,
        /// Print only the bash key bindings (inputrc format)
        #[arg(long, hide = true)]
        inputrc: bool,
    },
    /// Open the settings app (AI provider, theme, display, rules browser)
    Setup,
    /// Explain a command in detail, with the AI review if configured
    Explain {
        #[arg(long)]
        json: bool,
        /// Skip the AI review
        #[arg(long)]
        no_ai: bool,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        command: Vec<String>,
    },
    /// Check the installation, fonts, colors and AI connection
    Doctor,
    /// Send a sample command to the configured AI and show the result
    TestAi,
    /// Install or check the recommended Nerd Fonts
    Fonts {
        #[command(subcommand)]
        cmd: FontsCmd,
    },
    /// Measure speed on this machine against the performance budgets
    Bench {
        /// Also time real keystrokes through a pseudo-terminal
        #[arg(long)]
        pty: bool,
        /// Exit non-zero when a budget is missed (for CI)
        #[arg(long)]
        strict: bool,
        /// Multiply the time budgets, for noisy shared machines such as CI
        #[arg(long, default_value_t = 1.0)]
        slack: f64,
    },
    /// Remove shell hooks, cache, config and fonts installed by BeforeRunIt
    Uninstall {
        #[arg(long, short)]
        yes: bool,
        /// Keep config.toml and rules
        #[arg(long)]
        keep_config: bool,
    },
    /// Store or remove the AI API key (system keyring, falls back to a 0600 file)
    Key {
        #[command(subcommand)]
        cmd: KeyCmd,
    },
    /// Control the background daemon
    Daemon {
        #[command(subcommand)]
        cmd: Option<DaemonCmd>,
        /// Stay in the foreground and log to stderr
        #[arg(long)]
        foreground: bool,
    },
    /// Rebuild the config cache from config.toml, rules.d and themes
    Compile {
        #[arg(long, short)]
        quiet: bool,
    },
    /// Used by shell integrations: analyze a line and print the ghost line
    #[command(hide = true)]
    Query(query::QueryArgs),
}

#[derive(Subcommand)]
enum FontsCmd {
    /// Download and install Maple Mono NF (default) or JetBrainsMono NF
    Install {
        #[arg(value_enum, default_value_t = fonts::Font::Maple)]
        font: fonts::Font,
    },
    /// Show installed Nerd Fonts and terminal setup hints
    Status,
}

#[derive(Subcommand)]
enum KeyCmd {
    /// Read the key from stdin (or prompt) and store it
    Set,
    /// Remove the stored key
    Delete,
    /// Show where the key is stored
    Status,
}

#[derive(Subcommand)]
enum DaemonCmd {
    /// Start in the background (default)
    Start,
    Stop,
    Status,
    /// Reload config and the AI client
    Reload,
}

fn main() {
    let cli = Cli::parse();
    let code = match cli.cmd {
        Cmd::Query(a) => query::run(a),
        Cmd::Init { shell, no_builtin, inputrc } => init::run(shell, no_builtin, inputrc),
        Cmd::Setup => setup::run(),
        Cmd::Explain { json, no_ai, command } => explain::run(&command.join(" "), json, no_ai),
        Cmd::Doctor => doctor::run(),
        Cmd::TestAi => testai::run(),
        Cmd::Fonts { cmd } => match cmd {
            FontsCmd::Install { font } => fonts::install(font),
            FontsCmd::Status => fonts::status(),
        },
        Cmd::Bench { pty, strict, slack } => bench::run(pty, strict, slack),
        Cmd::Uninstall { yes, keep_config } => uninstall::run(yes, keep_config),
        Cmd::Key { cmd } => match cmd {
            KeyCmd::Set => keys::cli_set(),
            KeyCmd::Delete => keys::cli_delete(),
            KeyCmd::Status => keys::cli_status(),
        },
        Cmd::Daemon { cmd, foreground } => match cmd {
            None | Some(DaemonCmd::Start) => daemon::start(foreground),
            Some(DaemonCmd::Stop) => daemon::ctl("STOP"),
            Some(DaemonCmd::Status) => daemon::ctl("STATS"),
            Some(DaemonCmd::Reload) => daemon::ctl("RELOAD"),
        },
        Cmd::Compile { quiet } => {
            let c = beforerunit_core::store::compile();
            if let Err(e) = beforerunit_core::store::save(&c) {
                eprintln!("beforerunit: cannot write cache: {e}");
            }
            if !quiet {
                for e in &c.errors {
                    eprintln!("beforerunit: {e}");
                }
                println!("Compiled {} user rules and {} themes into {}", c.cmds.len(), c.themes.len(), beforerunit_core::store::compiled_file().display());
            }
            let _ = beforerunit_core::proto::send("RELOAD");
            i32::from(!c.errors.is_empty())
        }
    };
    std::process::exit(code);
}
