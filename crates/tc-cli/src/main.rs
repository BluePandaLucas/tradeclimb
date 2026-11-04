use anyhow::{Result, ensure};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tc_core::*;
use tc_service::{Period, Store, Submission};

#[derive(Parser)]
#[command(
    name = "tc",
    version,
    about = "TradeClimb · local crypto research and paper trading"
)]
struct Args {
    #[arg(long, default_value = "var")]
    data_dir: PathBuf,
    #[command(subcommand)]
    command: Action,
}
#[derive(Subcommand)]
enum Action {
    Serve {
        #[arg(long, default_value = "127.0.0.1:8080")]
        bind: String,
        #[arg(long, default_value = "dist")]
        assets: PathBuf,
        #[arg(long, default_value_t = 2)]
        workers: usize,
    },
    Import {
        #[arg(long, default_value = "2022-10-31")]
        start: String,
        #[arg(long, default_value = "2026-11-01")]
        end: String,
        #[arg(long)]
        accept_gaps: bool,
    },
    Demo,
    Register {
        #[arg(long)]
        name: String,
        #[arg(long)]
        description: String,
        #[arg(long)]
        executable: PathBuf,
        #[arg(long)]
        project: PathBuf,
        #[arg(long)]
        parent: Option<String>,
        #[arg(long, default_value = "{}")]
        parameters: String,
    },
    Builtin {
        #[arg(long)]
        name: String,
        #[arg(long, default_value = "ma")]
        kind: String,
        #[arg(long, default_value_t = 20)]
        fast: usize,
        #[arg(long, default_value_t = 100)]
        slow: usize,
    },
    Campaign {
        dataset: String,
        #[arg(long, default_value_t = 0.15)]
        target_cagr: f64,
        #[arg(long, default_value = "sharpe_daily_v1", value_parser = ["sharpe_daily_v1", "net_cagr"])]
        objective: String,
    },
    Run {
        #[arg(long)]
        strategy: String,
        #[arg(long)]
        campaign: String,
        #[arg(long)]
        validation: bool,
        #[arg(long)]
        key: Option<String>,
        #[arg(long)]
        slippage_bps: Option<u32>,
    },
    Inspect {
        id: String,
        #[arg(long)]
        reveal_validation: bool,
        #[arg(long)]
        output: PathBuf,
    },
    Budget {
        #[arg(long)]
        extend: Option<u32>,
    },
    Backup {
        destination: PathBuf,
    },
    List,
    #[command(hide = true)]
    Worker {
        id: String,
    },
}
fn print(value: &impl serde::Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
fn main() -> Result<()> {
    let args = Args::parse();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let store = Store::open(&args.data_dir)?;
    match args.command {
        Action::Worker { id } => {
            uuid_check(&id)?;
            tc_service::worker(&store.root, &id)
        }
        Action::Serve {
            bind,
            assets,
            workers,
        } => {
            ensure!(
                assets.join("index.html").exists(),
                "web assets missing; run ./scripts/build-web first"
            );
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?;
            let result = runtime.block_on(tc_service::serve(store, &bind, &assets, workers));
            runtime.shutdown_timeout(std::time::Duration::from_secs(2));
            result
        }
        Action::Import {
            start,
            end,
            accept_gaps,
        } => {
            let manifest = tc_data::import(
                &store.root,
                timestamp(&start)?,
                timestamp(&end)?,
                accept_gaps,
            )?;
            print(&manifest)
        }
        Action::Demo => {
            let manifest = tc_data::demo(&store.root)?;
            let campaign = store.campaign(&manifest.id)?;
            let mut strategies = vec![];
            for (name, kind) in [
                ("Cash reference", StrategyKind::Cash),
                ("Crypto basket", StrategyKind::BuyHold),
                (
                    "Trend explorer",
                    StrategyKind::MovingAverage {
                        fast: 20,
                        slow: 100,
                    },
                ),
            ] {
                let spec = StrategySpec {
                    parameters: Default::default(),
                    name: name.into(),
                    description:
                        "Platform example. Synthetic fixture results are not market evidence."
                            .into(),
                    kind,
                    markets: MARKETS.iter().map(|m| (*m).into()).collect(),
                };
                let s = store.register(spec, None, None)?;
                let j = store.submit(Submission {
                    strategy: s.id.clone(),
                    campaign: campaign.id.clone(),
                    period: Period::Development,
                    idempotency_key: format!("demo:{}:{}", campaign.id, s.id),
                    slippage_bps: None,
                })?;
                strategies.push(serde_json::json!({"strategy":s.id,"job":j.id}));
            }
            print(
                &serde_json::json!({"dataset":manifest.id,"campaign":campaign.id,"strategies":strategies,"warning":"SYNTHETIC fixture only; run import for Bitvavo research"}),
            )
        }
        Action::Register {
            name,
            description,
            executable,
            project,
            parent,
            parameters,
        } => {
            let project = std::fs::canonicalize(project)?;
            ensure!(
                project.join("Cargo.lock").exists(),
                "strategy project must include Cargo.lock"
            );
            let path = std::fs::canonicalize(executable)?.to_string_lossy().into();
            let spec = StrategySpec {
                parameters: serde_json::from_str(&parameters)?,
                name,
                description,
                kind: StrategyKind::Executable { path },
                markets: MARKETS.iter().map(|m| (*m).into()).collect(),
            };
            print(&store.register(spec, parent, Some(&project))?)
        }
        Action::Builtin {
            name,
            kind,
            fast,
            slow,
        } => {
            let kind = match kind.as_str() {
                "cash" => StrategyKind::Cash,
                "hold" => StrategyKind::BuyHold,
                "ma" => StrategyKind::MovingAverage { fast, slow },
                _ => anyhow::bail!("kind must be cash, hold, or ma"),
            };
            print(&store.register(
                StrategySpec {
                    parameters: Default::default(),
                    name,
                    description: "Built-in strategy fixture".into(),
                    kind,
                    markets: MARKETS.iter().map(|m| (*m).into()).collect(),
                },
                None,
                None,
            )?)
        }
        Action::Campaign {
            dataset,
            target_cagr,
            objective,
        } => print(&store.campaign_with_objective(
            &dataset,
            target_cagr,
            serde_json::from_value(serde_json::json!(objective))?,
        )?),
        Action::Run {
            strategy,
            campaign,
            validation,
            key,
            slippage_bps,
        } => print(&store.submit(Submission {
            strategy,
            campaign,
            period: if validation {
                Period::Validation
            } else {
                Period::Development
            },
            idempotency_key: key.unwrap_or_else(|| format!("cli:{}", now())),
            slippage_bps,
        })?),
        Action::Inspect {
            id,
            reveal_validation,
            output,
        } => {
            let job = store.job(&id)?;
            ensure!(
                job.period != Period::Validation || reveal_validation,
                "explicit --reveal-validation required; this marks the validation ledger revealed"
            );
            let report = store.owner_report(&id)?;
            tc_data::save_json(&output, &report)?;
            println!("Report saved to {}", output.display());
            Ok(())
        }
        Action::Budget { extend } => {
            if let Some(n) = extend {
                store.extend_budget(n)?;
            }
            print(&store.ledger()?)
        }
        Action::Backup { destination } => {
            store.backup(&destination)?;
            println!("Backup saved to {}", destination.display());
            Ok(())
        }
        Action::List => print(
            &serde_json::json!({"strategies":store.objects::<tc_service::RegisteredStrategy>("strategy")?,"campaigns":store.objects::<tc_service::Campaign>("campaign")?,"jobs":store.jobs()?,"datasets":tc_data::list(&store.root)?,"validation":store.ledger()?}),
        ),
    }
}
fn uuid_check(id: &str) -> Result<()> {
    ensure!(
        id.len() == 36 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'),
        "invalid worker id"
    );
    Ok(())
}
