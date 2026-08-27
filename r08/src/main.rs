use std::fs::File;
use std::io::{self, Write};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use clap::{Parser, Subcommand};
use r08::ble::{self, has_adapter};
use r08::identity::RING_NAME;
use r08::imu_scroll::{self, ImuStreamOptions, ImuWheelConfig, RotationPlane};
use r08::ota::{self, OtaFetchOptions, OtaRegion};
use r08::platform;
use r08::protocol::{find_device_packet, parse_hex_payload, reject_if_dfu, NORDIC_UART_WRITE};
use r08::sensor::{self, SensorRecordOptions};
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
    /// Verify a bidirectional GATT connection without enabling sensors or input control.
    ConnectCheck,
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
    /// Record the known A1 03 accelerometer stream to CSV. Never sends DFU.
    SensorRecord {
        #[arg(long, default_value_t = 30)]
        seconds: u64,
        #[arg(long)]
        output: Option<std::path::PathBuf>,
    },
    /// Send only the known A1 02 raw-sensor stop packet. Never sends DFU.
    SensorStop,
    /// Use the installed v7 A2/10 IMU stream. Does not flash firmware.
    ImuStream {
        /// Required acknowledgement; stock firmware does not implement A1 09.
        #[arg(long)]
        acknowledge_unverified_candidate: bool,
        /// Actually inject wheel events. Omit for listen/calibration-only mode.
        #[arg(long)]
        inject: bool,
        /// Keep the v7 IMU stream in standby and require two distinct knocks before enabling control for one minute.
        #[arg(long)]
        double_tap_wake: bool,
        #[arg(long, default_value_t = 60)]
        seconds: u64,
        /// Gravity rotation plane: xy, xz, or yz.
        #[arg(long, default_value = "xz")]
        plane: String,
        #[arg(long)]
        invert: bool,
        #[arg(long, default_value_t = 6.0)]
        deadzone: f64,
        #[arg(long, default_value_t = 40.0)]
        full_speed: f64,
        #[arg(long, default_value_t = 1.0)]
        gain: f64,
    },
    /// Query the official QRing OTA service and optionally download an exact matching image. Never sends DFU.
    OtaFetch {
        /// Use the global OTA service instead of the default mainland China service.
        #[arg(long)]
        global: bool,
        /// Only print matching metadata; do not download the firmware file.
        #[arg(long)]
        metadata_only: bool,
        /// Skip the device-information confirmation prompt.
        #[arg(long)]
        yes: bool,
        /// Use an existing token instead of the default QRing guest token.
        #[arg(long)]
        token_auth: bool,
        /// Log in with a QRing account instead of the default QRing guest token.
        #[arg(long)]
        account_auth: bool,
        #[arg(long, default_value = ota::DEFAULT_HARDWARE_VERSION)]
        hardware_version: String,
        #[arg(long, default_value = ota::DEFAULT_ROM_VERSION)]
        rom_version: String,
        /// Advanced: report an older installed version to retrieve the latest package; the result must still match --rom-version.
        #[arg(long)]
        query_rom_version: Option<String>,
        #[arg(long, default_value = ota::DEFAULT_MAC)]
        mac: String,
        #[arg(long, default_value = "CN")]
        country: String,
        #[arg(long, default_value = "firmware_research/evidence/ota")]
        output_dir: std::path::PathBuf,
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
    /// Send one official 0x50 touch-area indicator request and exit.
    TouchIndicatorTest,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let command = cli.command.unwrap_or_else(default_command);
    let logging = matches!(
        &command,
        Command::ConnectCheck
            | Command::Control { .. }
            | Command::Interactive { .. }
            | Command::ImuStream { .. }
    );
    init_tracing(logging)?;
    match command {
        Command::ConnectCheck => {
            println!("正在建立最小稳定连接：不会启动提示灯、心率/血氧灯、滚轮、IMU 或触控映射。");
            let connection = ble::connect(30).await?;
            let result = connection.verify_uart_round_trip().await;
            let disconnect_result = connection.disconnect().await;
            let battery_percent = result?;
            disconnect_result?;
            println!("CONNECT_STABLE GATT 双向收发验证成功，电量={battery_percent}%");
            println!("TOUCH_INDICATOR_SKIPPED 原厂 50 55 AA 是约 20 次的查找设备长闪烁，默认连接检查不再发送");
            Ok(())
        }
        Command::TouchIndicatorTest => {
            println!(
                "正在发送一次已知的 50 55 AA 触控区提示命令；不会启动心率/血氧灯、IMU 或输入注入。"
            );
            let connection = ble::connect(30).await?;
            connection.write(&find_device_packet()).await?;
            println!("TOUCH_INDICATOR_SENT 请观察触控区域；v8 候选应闪烁约 3 次");
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            connection.disconnect().await?;
            Ok(())
        }
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
                    require_double_tap_wake: false,
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
        Command::SensorRecord { seconds, output } => {
            println!("正在查找并连接 {RING_NAME}，最多等待 30 秒……");
            println!("此测试会发送已知的 A1 04 04 原始传感器命令，LED 可能闪烁；不会发送 DFU。");
            let connection = ble::connect_fresh(30).await?;
            let result = sensor::record(&connection, SensorRecordOptions { seconds, output }).await;
            let disconnect_result = connection.disconnect().await;
            match result {
                Ok(summary) => {
                    disconnect_result?;
                    println!("SENSOR_SUMMARY {}", serde_json::to_string(&summary)?);
                    Ok(())
                }
                Err(error) => {
                    let _ = disconnect_result;
                    Err(error)
                }
            }
        }
        Command::SensorStop => {
            println!("正在查找并连接 {RING_NAME}，最多等待 20 秒……");
            let connection = ble::connect(20).await?;
            let result = sensor::stop(&connection).await;
            let disconnect_result = connection.disconnect().await;
            result?;
            disconnect_result?;
            println!("已发送 A1 02，原始传感器模式应已关闭");
            Ok(())
        }
        Command::ImuStream {
            acknowledge_unverified_candidate,
            inject,
            double_tap_wake,
            seconds,
            plane,
            invert,
            deadzone,
            full_speed,
            gain,
        } => {
            if !acknowledge_unverified_candidate {
                anyhow::bail!(
                    "拒绝发送候选命令：必须显式添加 --acknowledge-unverified-candidate；此命令不刷固件，但仅适用于已验证安装候选固件的测试设备"
                );
            }
            let config = ImuWheelConfig {
                plane: plane.parse::<RotationPlane>()?,
                invert,
                deadzone_degrees: deadzone,
                full_speed_degrees: full_speed,
                gain,
            }
            .validate()?;
            println!("正在查找并连接 {RING_NAME}，最多等待 30 秒……");
            if double_tap_wake {
                println!("组合模式不会刷写固件，也不会发送会切断 GATT 的 3B 触控命令；待机 IMU 只检测双敲，确认两次后开启 60 秒转动滚动。");
            } else {
                println!("该命令不会刷写固件；将发送候选 A1 09 启停命令，任何异常均急停。按 Enter 或 Ctrl+C 退出。");
            }
            let connection = ble::connect(30).await?;
            imu_scroll::run(
                connection,
                ImuStreamOptions {
                    seconds,
                    inject,
                    double_tap_wake,
                    config,
                },
            )
            .await
        }
        Command::OtaFetch {
            global,
            metadata_only,
            yes,
            token_auth,
            account_auth,
            hardware_version,
            rom_version,
            query_rom_version,
            mac,
            country,
            output_dir,
        } => {
            ota::fetch(OtaFetchOptions {
                region: if global {
                    OtaRegion::Global
                } else {
                    OtaRegion::China
                },
                hardware_version,
                rom_version,
                query_rom_version,
                mac,
                country,
                output_dir,
                metadata_only,
                assume_yes: yes,
                token_auth,
                account_auth,
            })
            .await
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
                    require_double_tap_wake: false,
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
                    require_double_tap_wake: true,
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
    Command::ConnectCheck
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
    println!("inject_default=double-tap-gated");
    println!("dfu_writes=blocked");
    println!("target_name={RING_NAME}");
    println!("firmware_images=not-bundled");
    println!("firmware_recovery_path=not-verified");
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
    use clap::Parser;

    use super::{default_command, Cli, Command};

    #[test]
    fn default_command_only_checks_connection() {
        assert!(matches!(default_command(), Command::ConnectCheck));
    }

    #[test]
    fn connect_check_subcommand_is_available() {
        let cli = Cli::try_parse_from(["r08", "connect-check"]).unwrap();
        assert!(matches!(cli.command, Some(Command::ConnectCheck)));
    }

    #[test]
    fn imu_stream_accepts_double_tap_wake_launcher_flags() {
        let cli = Cli::try_parse_from([
            "r08",
            "imu-stream",
            "--acknowledge-unverified-candidate",
            "--inject",
            "--double-tap-wake",
            "--seconds",
            "0",
            "--gain",
            "0.2",
            "--full-speed",
            "60",
        ])
        .unwrap();
        match cli.command.unwrap() {
            Command::ImuStream {
                acknowledge_unverified_candidate,
                inject,
                double_tap_wake,
                seconds,
                gain,
                full_speed,
                ..
            } => {
                assert!(acknowledge_unverified_candidate);
                assert!(inject);
                assert!(double_tap_wake);
                assert_eq!(seconds, 0);
                assert_eq!(gain, 0.2);
                assert_eq!(full_speed, 60.0);
            }
            _ => panic!("launcher flags must select imu-stream"),
        }
    }
}
