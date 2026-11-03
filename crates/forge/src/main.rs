use clap::Parser;

/// High-performance terminal git + file manager.
#[derive(Debug, Parser)]
#[command(name = "forge", version, about)]
struct Cli {
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

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    forge_app::run_with_options(forge_app::RunOptions {
        force_truecolor: cli.force_truecolor,
        force_ansi256: cli.force_ansi256,
        force_ansi16: cli.force_ansi16,
        demo_colors: cli.demo_colors,
    })
}
