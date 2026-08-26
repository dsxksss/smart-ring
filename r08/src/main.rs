use std::fs::File;
use std::io::{self, Write};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use clap::{Parser, Subcommand};
use r08::ble::{self, has_adapter};
use r08::identity::RING_NAME;
use r08::platform;
use r08::protocol::{parse_hex_payload, reject_if_dfu, NORDIC_UART_WRITE};
use r08::session::{self, SessionOptions};
use tracing_subscriber::fmt::MakeWriter;

#[derive(Parser)]
#[command(
    name = "r08",
    about = "Cross-platform R08 smart ring controller (Windows / Linux / macOS)",
    disable_help_subcommand = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Open the advanced numeric diagnostic menu.
    #[command(alias = "shell")]
    Interactive {
        #[arg(long, default_value_t = 2)]
        touch_type: u8,
        #[arg(long, default_value_t = 1)]
        sleep_minutes: u8,
        #[arg(long, default_value_t = 4)]
        scroll_gain: i32,
    },
    /// Print OS/BLE/HID/inject support without touching the ring.
    SelfCheck,
    /// Scan for R08_9C07.
    Scan {
        #[arg(long, default_value_t = 8)]
        seconds: u64,
    },
    /// Read Device Information Service only. No writes.
    DeviceInfo {
        #[arg(long, default_value_t = 20)]
        seconds: u64,
    },
    /// Enable touch notifications and log packets. Does not inject.
    Listen {
        #[arg(long, default_value_t = 0)]
        seconds: u64,
        #[arg(long, default_value_t = 2)]
        touch_type: u8,
        #[arg(long, default_value_t = 1)]
        sleep_minutes: u8,
    },
    /// Enable host injection: vertical HID/GATT swipe to wheel, double-click copy, triple-click paste.
    Control {
        #[arg(long, default_value_t = 0)]
        seconds: u64,
        #[arg(long, default_value_t = 2)]
        touch_type: u8,
        #[arg(long, default_value_t = 1)]
        sleep_minutes: u8,
        #[arg(long, default_value_t = 4)]
        scroll_gain: i32,
    },
    /// Write the official 0x3B disable packet and exit.
    DisableTouch {
        #[arg(long, default_value_t = 1)]
        sleep_minutes: u8,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let command = cli.command.unwrap_or_else(default_command);
    let logging = matches!(
        &command,
        Command::Control { .. } | Command::Interactive { .. }
    );
    init_tracing(logging)?;
    match command {
        Command::Interactive {
            touch_type,
            sleep_minutes,
            scroll_gain,
        } => {
            println!("正在查找并连接 {RING_NAME}，最多等待 30 秒……");
            let connection = ble::connect(30).await?;
            session::run(
                connection,
                SessionOptions {
                    inject: false,
                    touch_on_start: false,
                    touch_type,
                    sleep_minutes,
                    scroll_gain,
                    seconds: 0,
                    interactive_menu: true,
                },
            )
            .await
        }
        Command::SelfCheck => self_check().await,
        Command::Scan { seconds } => {
            let found = ble::scan(seconds).await?;
            if found.is_empty() {
                println!("没有发现 {RING_NAME}");
            } else {
                for (name, address) in found {
                    println!("{name}  {address}");
                }
            }
            Ok(())
        }
        Command::DeviceInfo { seconds } => {
            let connection = ble::connect(seconds).await?;
            println!(
                "DEVICE_INFO_READ_ONLY {} {}",
                connection.name, connection.address
            );
            for (label, value) in connection.read_device_information().await? {
                println!("{label}: {value}");
            }
            connection.disconnect().await?;
            Ok(())
        }
        Command::Listen {
            seconds,
            touch_type,
            sleep_minutes,
        } => {
            println!("正在查找并连接 {RING_NAME}，最多等待 30 秒……");
            let connection = ble::connect(30).await?;
            session::run(
                connection,
                SessionOptions {
                    inject: false,
                    touch_on_start: true,
                    touch_type,
                    sleep_minutes,
                    scroll_gain: 4,
                    seconds,
                    interactive_menu: false,
                },
            )
            .await
        }
        Command::Control {
            seconds,
            touch_type,
            sleep_minutes,
            scroll_gain,
        } => {
            println!("正在查找并连接 {RING_NAME}，最多等待 30 秒……");
            let connection = ble::connect(30).await?;
            session::run(
                connection,
                SessionOptions {
                    inject: true,
                    touch_on_start: true,
                    touch_type,
                    sleep_minutes,
                    scroll_gain,
                    seconds,
                    interactive_menu: false,
                },
            )
            .await
        }
        Command::DisableTouch { sleep_minutes } => {
            reject_if_dfu(NORDIC_UART_WRITE)?;
            let connection = ble::connect(20).await?;
            connection
                .write(&r08::protocol::touch_disable_packet(sleep_minutes))
                .await?;
            connection.disconnect().await?;
            println!("已发送关闭触控命令");
            Ok(())
        }
    }
}

fn default_command() -> Command {
    Command::Control {
        seconds: 0,
        touch_type: 2,
        sleep_minutes: 1,
        scroll_gain: 4,
    }
}

async fn self_check() -> Result<()> {
    let caps = platform::capabilities();
    println!("os={}", caps.os);
    println!("ble_backend={}", caps.ble_backend);
    println!("hid_backend={}", caps.hid_backend);
    println!("inject_backend={}", caps.inject_backend);
    println!(
        "ble_adapter={}",
        if has_adapter().await {
            "present"
        } else {
            "not-found"
        }
    );
    println!("inject_default=on");
    println!("dfu_writes=blocked");
    println!("target_name={RING_NAME}");
    println!("firmware_backup=not-present");
    for note in caps.notes {
        println!("note={note}");
    }
    let _ = parse_hex_payload("3B 02 00 02 01")?;
    println!("protocol=ok");
    Ok(())
}

fn init_tracing(log_file: bool) -> Result<()> {
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    if log_file {
        let path = "r08-control-latest.log";
        let file = File::create(path)?;
        println!("LOG_FILE {path}");
        tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .with_writer(TeeWriter::new(file))
            .init();
    } else {
        tracing_subscriber::fmt().with_env_filter(env_filter).init();
    }
    Ok(())
}

struct TeeWriter {
    file: Arc<Mutex<File>>,
}

impl TeeWriter {
    fn new(file: File) -> Self {
        Self {
            file: Arc::new(Mutex::new(file)),
        }
    }
}

impl Clone for TeeWriter {
    fn clone(&self) -> Self {
        Self {
            file: Arc::clone(&self.file),
        }
    }
}

impl Write for TeeWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        io::stdout().write_all(buf)?;
        self.file.lock().unwrap().write_all(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        io::stdout().flush()?;
        self.file.lock().unwrap().flush()
    }
}

impl<'a> MakeWriter<'a> for TeeWriter {
    type Writer = TeeWriter;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::{default_command, Command};

    #[test]
    fn default_command_starts_double_tap_wake_control() {
        match default_command() {
            Command::Control {
                seconds,
                touch_type,
                sleep_minutes,
                scroll_gain,
            } => {
                assert_eq!(seconds, 0);
                assert_eq!(touch_type, 2);
                assert_eq!(sleep_minutes, 1);
                assert_eq!(scroll_gain, 4);
            }
            _ => panic!("default command must start control directly"),
        }
    }
}
