use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::Parser;
use r08::sacrificial_dfu::{self, ACK_PHRASE, EXPECTED_SHA256};

#[derive(Debug, Parser)]
#[command(about = "One-device, hash-locked sacrificial R08 v9 DFU")]
struct Args {
    #[arg(long, conflicts_with_all = ["dry_run", "execute_sacrificial_test", "activation_probe"])]
    probe: bool,
    #[arg(long, conflicts_with_all = ["probe", "execute_sacrificial_test", "activation_probe"])]
    dry_run: bool,
    #[arg(long, conflicts_with_all = ["probe", "dry_run", "activation_probe"])]
    execute_sacrificial_test: bool,
    #[arg(long, conflicts_with_all = ["probe", "dry_run", "execute_sacrificial_test"])]
    activation_probe: bool,
    #[arg(long)]
    candidate: Option<PathBuf>,
    #[arg(long, default_value = "")]
    ack: String,
    #[arg(long, default_value = "")]
    ack_sha256: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let modes = usize::from(args.probe)
        + usize::from(args.dry_run)
        + usize::from(args.execute_sacrificial_test)
        + usize::from(args.activation_probe);
    if modes != 1 {
        bail!(
            "select exactly one of --probe, --dry-run, --execute-sacrificial-test, --activation-probe"
        );
    }
    if args.probe {
        let (battery, hardware, firmware) = sacrificial_dfu::probe().await?;
        println!(
            "PROBE_OK name=R08_9C07 address=31:31:45:37:9C:07 hardware={hardware} firmware={firmware} battery={battery}% dfu_service=present"
        );
        return Ok(());
    }
    if args.activation_probe {
        let response = sacrificial_dfu::probe_activation_marker().await?;
        if response.get(..2) == Some(&[0xA2, 0x10]) {
            let sequence = response.get(2).copied().unwrap_or_default();
            println!("IMU_STREAM_ACTIVE sequence={sequence}");
        } else {
            let status = response.get(1).copied().unwrap_or_default();
            println!("ACTIVATION_MARKER 0x{status:02X}");
        }
        return Ok(());
    }
    let path = args.candidate.context("--candidate is required")?;
    let candidate = sacrificial_dfu::load_candidate(&path)?;
    println!(
        "LOCKED candidate={} bytes={} crc16=0x{:04X} sum16=0x{:04X} blocks={}",
        candidate.sha256,
        candidate.bytes.len(),
        candidate.crc16,
        candidate.sum16,
        candidate.blocks
    );
    if args.dry_run {
        return Ok(());
    }
    if args.ack != ACK_PHRASE {
        bail!("exact permanent-brick acknowledgement is required");
    }
    if args.ack_sha256.to_ascii_lowercase() != EXPECTED_SHA256 {
        bail!("acknowledged SHA-256 does not match the locked candidate");
    }
    sacrificial_dfu::execute(&candidate).await
}
