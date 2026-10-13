use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use edge_purge_sync::cloudflare::{CloudflareClient, PurgeTarget};
use edge_purge_sync::config::Config;
use edge_purge_sync::daemon;
use edge_purge_sync::error::{Error, Result};
use edge_purge_sync::state::State;
use edge_purge_sync::sync::{self, SyncOptions};

#[derive(Parser)]
#[command(
    name = "edge-purge-sync",
    version,
    about = "Autonomous Cloudflare/CDN cache synchronizer: diffs built asset hashes against edge headers and triggers granular Cache API purges"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,

    /// Config file path
    #[arg(short, long, global = true, default_value = "edge-purge.toml")]
    config: PathBuf,

    /// Report only, never call the purge API
    #[arg(long, global = true)]
    dry_run: bool,

    /// Machine readable JSON report on stdout
    #[arg(long, global = true)]
    json: bool,

    /// Exit 3 when purgeable drift remains
    #[arg(long, global = true)]
    fail_on_drift: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Diff local asset hashes against edge state and purge drifted URLs
    Sync,
    /// Diff only: report drift, no purge, no state write
    Scan,
    /// Run periodic sync cycles until interrupted
    Daemon {
        /// Cycle interval in seconds (defaults to daemon.interval_secs)
        #[arg(long)]
        interval: Option<u64>,
    },
    /// Manual granular purge
    Purge {
        #[arg(long = "url")]
        urls: Vec<String>,
        #[arg(long = "tag")]
        tags: Vec<String>,
        #[arg(long = "host")]
        hosts: Vec<String>,
        #[arg(long)]
        everything: bool,
    },
    /// Show persisted sync state
    Status,
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();
    match run().await {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<ExitCode> {
    let cli = Cli::parse();
    let cfg = Config::load(&cli.config)?;
    match cli.command {
        Command::Sync | Command::Scan => {
            let opts = SyncOptions {
                dry_run: cli.dry_run,
                purge: matches!(cli.command, Command::Sync),
            };
            let report = sync::run(&cfg, &opts).await?;
            emit(&report, cli.json);
            if cli.fail_on_drift && report.has_purgeable_drift() && !report.purged {
                return Ok(ExitCode::from(3));
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Daemon { interval } => {
            daemon::run(&cfg, interval, cli.dry_run).await?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Purge {
            urls,
            tags,
            hosts,
            everything,
        } => {
            if urls.is_empty() && tags.is_empty() && hosts.is_empty() && !everything {
                return Err(Error::Config(
                    "purge needs --url, --tag, --host or --everything".into(),
                ));
            }
            let cf = CloudflareClient::from_config(&cfg)?;
            let target = if everything {
                PurgeTarget::Everything
            } else if !urls.is_empty() {
                PurgeTarget::Urls(urls)
            } else if !tags.is_empty() {
                PurgeTarget::Tags(tags)
            } else {
                PurgeTarget::Hosts(hosts)
            };
            let batches = cf.purge(&target, &cfg.purge).await?;
            if cli.json {
                println!("{{\"purged\":true,\"batches\":{batches}}}");
            } else {
                println!("purged batches={batches}");
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Status => {
            let state = State::load(&cfg.state.path)?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&state)?);
            } else {
                println!(
                    "state {} updated_at={} assets={}",
                    cfg.state.path.display(),
                    state.updated_at,
                    state.assets.len()
                );
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn emit(report: &sync::SyncReport, json: bool) {
    if json {
        match serde_json::to_string_pretty(report) {
            Ok(text) => println!("{text}"),
            Err(err) => eprintln!("error: {err}"),
        }
    } else {
        print!("{}", sync::render_human(report));
    }
}
