use clap::{Parser, Subcommand};

mod update;

/// High-performance terminal git + file manager.
#[derive(Debug, Parser)]
#[command(name = "forge", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Force named 16-color VGA palette (default; works on basic GNOME Terminal).
    #[arg(long)]
    force_ansi16: bool,

    /// Force ANSI 256 indexed colors.
    #[arg(long)]
    force_ansi256: bool,

    /// Force 24-bit Color::Rgb (sets COLORTERM=truecolor for this process).
    #[arg(long)]
    force_truecolor: bool,

    /// Print colored smoke lines then exit (no TUI). Use to verify ANSI colors.
    #[arg(long)]
    demo_colors: bool,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Update Forge in place from the latest GitHub release.
    Update {
        /// Report whether a newer release exists without installing it.
        #[arg(long)]
        check: bool,

        /// Reinstall the latest release even when the version already matches.
        #[arg(long)]
        force: bool,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    if let Some(command) = cli.command {
        return match command {
            Command::Update { check, force } => update::run(check, force),
        };
    }
    forge_app::run_with_options(forge_app::RunOptions {
        force_truecolor: cli.force_truecolor,
        force_ansi256: cli.force_ansi256,
        force_ansi16: cli.force_ansi16,
        demo_colors: cli.demo_colors,
    })
}
