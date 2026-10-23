//! `mc3000-mcp` binary: MCP stdio server plus read-only charger diagnostics.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use mc3000_mcp::client::ChargerClient;
use mc3000_mcp::mcp::{McpService, SharedTransport};
use mc3000_mcp::model::ChargerModel;
use mc3000_mcp::transport::{BleTransport, FakeTransport, Transport, timeout_secs};
use rmcp::{ServiceExt, transport::stdio};
use serde_json::json;
use std::process::ExitCode;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

/// Select the backend used by the MCP server and diagnostic commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum Backend {
    /// Real BLE hardware through `BlueZ`.
    Ble,
    /// Deterministic in-process backend for client integration tests.
    Fake,
}

#[derive(Debug, Parser)]
#[command(
    name = "mc3000-mcp",
    version,
    about = "Safe MCP server for SKYRC MC3000-compatible battery chargers over BLE"
)]
struct Cli {
    /// Backend used for this invocation.
    #[arg(long, value_enum, default_value_t = Backend::Ble, global = true)]
    backend: Backend,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Scan for charger-like BLE devices.
    Scan {
        /// Scan window in seconds.
        #[arg(long, default_value_t = 5.0)]
        timeout: f64,
    },
    /// Connect, read all four slots, print JSON, and disconnect.
    Status {
        /// Charger address. Omit to connect to the first discovered charger.
        #[arg(long)]
        address: Option<String>,
        /// Discovery scan window in seconds.
        #[arg(long, default_value_t = 5.0)]
        timeout: f64,
    },
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    let cli = Cli::parse();
    match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to build the Tokio runtime")
    {
        Ok(runtime) => match runtime.block_on(run(cli)) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("mc3000-mcp: {error:#}");
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            eprintln!("mc3000-mcp: {error:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Some(command) => run_command(cli.backend, command).await,
        None => serve(cli.backend).await,
    }
}

async fn make_transport(backend: Backend) -> Result<Arc<dyn Transport>> {
    match backend {
        Backend::Fake => Ok(Arc::new(FakeTransport::new())),
        Backend::Ble => Ok(Arc::new(
            BleTransport::new()
                .await
                .context("failed to initialize the BlueZ BLE backend")?,
        )),
    }
}

async fn serve(backend: Backend) -> Result<()> {
    let service = McpService::new(make_transport(backend).await?);
    let server = service
        .serve(stdio())
        .await
        .context("failed to start the MCP stdio server")?;
    server
        .waiting()
        .await
        .context("MCP stdio server terminated with an error")?;
    Ok(())
}

async fn run_command(backend: Backend, command: Command) -> Result<()> {
    let transport = SharedTransport::new(make_transport(backend).await?);
    let client = ChargerClient::new(transport);
    match command {
        Command::Scan { timeout } => {
            let duration = timeout_secs(timeout).context("invalid scan timeout")?;
            let devices = client.scan(duration).await?;
            let output = json!({
                "devices": devices,
                "matched_names": mc3000_mcp::protocol::DEVICE_NAMES,
            });
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        Command::Status { address, timeout } => {
            let duration = timeout_secs(timeout).context("invalid discovery timeout")?;
            let connected = client
                .find_and_connect(address.as_deref(), duration, ChargerModel::Mc3000)
                .await;
            let statuses = match connected {
                Ok(info) => match client.poll_all_status().await {
                    Ok(slots) => Some(json!({
                        "address": info.address,
                        "model": "mc3000",
                        "slots": slots,
                    })),
                    Err(error) => {
                        if let Err(disconnect_error) = client.disconnect().await {
                            tracing::warn!(%disconnect_error, "disconnect after status failure failed");
                        }
                        return Err(error.into());
                    }
                },
                Err(error) => return Err(error.into()),
            };
            if let Err(error) = client.disconnect().await {
                tracing::warn!(%error, "disconnect after status failed");
            }
            if let Some(output) = statuses {
                println!("{}", serde_json::to_string_pretty(&output)?);
            }
        }
    }
    Ok(())
}
