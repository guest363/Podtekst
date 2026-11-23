mod manifest;
mod metrics;
mod report;
mod runner;

use std::path::PathBuf;
use std::time::Duration;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "podtekst-eval",
    about = "Локальная диагностика словаря и hotwords"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Hotwords {
        #[command(subcommand)]
        command: Hotwords,
    },
}

#[derive(Subcommand)]
enum Hotwords {
    Run(RunArgs),
    Score(ScoreArgs),
}

#[derive(clap::Args)]
struct RunArgs {
    #[arg(long)]
    manifest: PathBuf,
    #[arg(long)]
    data_dir: PathBuf,
    #[arg(long)]
    out: PathBuf,
    #[arg(long)]
    app: Option<PathBuf>,
    #[arg(long, default_value = "off,on")]
    variants: String,
    #[arg(long, default_value = "gigaam")]
    engine: String,
    #[arg(long, default_value = "ru")]
    language: String,
    #[arg(long, default_value_t = 600)]
    timeout: u64,
}

#[derive(clap::Args)]
struct ScoreArgs {
    #[arg(long)]
    manifest: PathBuf,
    #[arg(long)]
    run: PathBuf,
    #[arg(long)]
    report: Option<PathBuf>,
}

fn main() -> Result<(), String> {
    match Cli::parse().command {
        Command::Hotwords { command } => match command {
            Hotwords::Run(args) => {
                let variants = args
                    .variants
                    .split(',')
                    .map(str::trim)
                    .filter(|variant| !variant.is_empty())
                    .map(str::to_string)
                    .collect::<Vec<_>>();
                let app = args.app.unwrap_or_else(runner::default_app);
                runner::run(runner::RunOptions {
                    manifest_path: &args.manifest,
                    data_dir: &args.data_dir,
                    app: &app,
                    out: &args.out,
                    variants: &variants,
                    engine: &args.engine,
                    language: &args.language,
                    timeout: Duration::from_secs(args.timeout),
                })
            }
            Hotwords::Score(args) => {
                let manifest = manifest::Manifest::load(&args.manifest)?;
                let base = args
                    .manifest
                    .parent()
                    .unwrap_or_else(|| std::path::Path::new("."));
                manifest.validate(base)?;
                report::score(&args.run, &manifest, args.report.as_deref())
            }
        },
    }
}
