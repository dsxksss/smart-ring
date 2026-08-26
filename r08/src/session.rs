use std::io::{self, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc as tokio_mpsc;

use crate::ble::RingConnection;
use crate::mapping::{InputEvent, MappingConfig, MappingEngine, Output};
use crate::platform::inject::{Injector, NullInjector};
use crate::platform::{create_injector, spawn_hid_monitor, PointerSuppression};
use crate::protocol::{touch_disable_packet, touch_enable_packet, touch_read_packet};

#[derive(Debug, Clone, PartialEq, Eq)]
enum MenuCommand {
    ListenOnly,
    Control,
    PauseControl,
    DisableTouch,
    Status,
    Help,
    Quit,
    Invalid(String),
}

pub struct SessionOptions {
    pub inject: bool,
    pub touch_on_start: bool,
    pub touch_type: u8,
    pub sleep_minutes: u8,
    pub scroll_gain: i32,
    pub seconds: u64,
    pub interactive_menu: bool,
}

pub async fn run(connection: RingConnection, options: SessionOptions) -> Result<()> {
    let mut pointer_suppression = PointerSuppression::new();
    pointer_suppression
        .suppress()
        .context("无法启用 R08 无光标移动保护；触控未开启")?;
    let mut notifications = connection.subscribe().await?;
    let mut touch_enabled = false;
    if options.touch_on_start {
        set_touch_enabled(&connection, &options, true).await?;
        touch_enabled = true;
    }

    let mut engine = MappingEngine::new(MappingConfig {
        scroll_gain: options.scroll_gain.clamp(1, 10),
        inject: options.inject,
    });
    let mut inject_enabled = options.inject;
    let mut injector: Box<dyn Injector> = if options.inject {
        create_injector().context("创建输入注入后端失败")?
    } else {
        Box::new(NullInjector)
    };

    let (hid_tx, hid_rx) = mpsc::channel();
    let _hid = spawn_hid_monitor(hid_tx).context("启动 HID 监听失败")?;
    if options.interactive_menu {
        tracing::info!(
            "MENU_READY 已连接 {} {}；输入数字选择功能",
            connection.name,
            connection.address
        );
        print_menu(touch_enabled, inject_enabled, &options);
    } else if inject_enabled {
        tracing::info!(
            "CONTROL_READY 已连接 {} {}；上下滑=滚轮，唤醒后双击=复制，三击=粘贴",
            connection.name,
            connection.address
        );
        println!("控制已开启：戒指休眠时双击唤醒；唤醒后上下滑滚动、双击复制、三击粘贴。");
        println!("按 Enter 或 Ctrl+C 安全退出。");
    } else {
        tracing::info!(
            "LISTEN_READY 已连接 {} {}；戒指休眠时双击唤醒，仅记录动作",
            connection.name,
            connection.address
        );
        println!("监听已开启：戒指休眠时双击唤醒；按 Enter 或 Ctrl+C 安全退出。");
    }

    let (command_tx, mut command_rx) = tokio_mpsc::channel::<MenuCommand>(8);
    if options.interactive_menu {
        tokio::spawn(async move {
            let mut stdin = BufReader::new(tokio::io::stdin());
            let mut line = String::new();
            loop {
                line.clear();
                match stdin.read_line(&mut line).await {
                    Ok(0) => {
                        let _ = command_tx.send(MenuCommand::Quit).await;
                        break;
                    }
                    Ok(_) => {
                        let command = parse_menu_command(&line);
                        let quit = command == MenuCommand::Quit;
                        if command_tx.send(command).await.is_err() || quit {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = command_tx
                            .send(MenuCommand::Invalid(format!("读取命令失败：{error}")))
                            .await;
                        break;
                    }
                }
            }
        });
    } else {
        tokio::spawn(async move {
            let mut stdin = BufReader::new(tokio::io::stdin());
            let mut line = String::new();
            match stdin.read_line(&mut line).await {
                Ok(0) => std::future::pending::<()>().await,
                Ok(_) => {
                    let _ = command_tx.send(MenuCommand::Quit).await;
                }
                Err(error) => {
                    tracing::warn!("读取退出命令失败：{error}");
                }
            }
        });
    }

    let started = Instant::now();
    let limit = (options.seconds > 0).then(|| Duration::from_secs(options.seconds));
    let mut tick = tokio::time::interval(Duration::from_millis(10));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut run_result = Ok(());

    loop {
        if limit.is_some_and(|duration| started.elapsed() >= duration) {
            break;
        }
        tokio::select! {
            _ = tick.tick() => {
                let now = started.elapsed().as_millis() as u64;
                if let Err(error) = apply_outputs(&mut injector, engine.tick(now)) {
                    run_result = Err(error);
                    break;
                }
                while let Ok(mouse) = hid_rx.try_recv() {
                    if let Err(error) = apply_outputs(
                        &mut injector,
                        engine.handle(InputEvent::HidMouse(mouse), now),
                    ) {
                        run_result = Err(error);
                        break;
                    }
                }
                if run_result.is_err() {
                    break;
                }
            }
            packet = futures::StreamExt::next(&mut notifications) => {
                let Some(packet) = packet else {
                    tracing::warn!("GATT 通知流结束");
                    break;
                };
                let now = started.elapsed().as_millis() as u64;
                if let Err(error) = apply_outputs(
                    &mut injector,
                    engine.handle(InputEvent::GattPacket(packet), now),
                ) {
                    run_result = Err(error);
                    break;
                }
            }
            command = command_rx.recv() => {
                let Some(command) = command else {
                    tracing::info!("命令输入已关闭，正在安全退出");
                    break;
                };
                match command {
                    MenuCommand::ListenOnly => {
                        disable_host_control(
                            &mut engine,
                            &mut injector,
                            &mut inject_enabled,
                        );
                        if !touch_enabled {
                            match set_touch_enabled(&connection, &options, true).await {
                                Ok(()) => touch_enabled = true,
                                Err(error) => tracing::warn!("开启触控失败：{error}"),
                            }
                        }
                        print_status(touch_enabled, inject_enabled, &options);
                    }
                    MenuCommand::Control => {
                        if !touch_enabled {
                            match set_touch_enabled(&connection, &options, true).await {
                                Ok(()) => touch_enabled = true,
                                Err(error) => tracing::warn!("开启触控失败：{error}"),
                            }
                        }
                        if touch_enabled && !inject_enabled {
                            match enable_host_control(
                                &mut engine,
                                &mut injector,
                                &mut inject_enabled,
                            ) {
                                Ok(()) => tracing::info!(
                                    "CONTROL_READY 上下滑=滚轮，双击=复制，三击=粘贴"
                                ),
                                Err(error) => tracing::warn!("开启电脑控制失败：{error}"),
                            }
                        }
                        print_status(touch_enabled, inject_enabled, &options);
                    }
                    MenuCommand::PauseControl => {
                        disable_host_control(
                            &mut engine,
                            &mut injector,
                            &mut inject_enabled,
                        );
                        tracing::info!("电脑控制已暂停；戒指触控监听保持不变");
                        print_status(touch_enabled, inject_enabled, &options);
                    }
                    MenuCommand::DisableTouch => {
                        disable_host_control(
                            &mut engine,
                            &mut injector,
                            &mut inject_enabled,
                        );
                        if touch_enabled {
                            match set_touch_enabled(&connection, &options, false).await {
                                Ok(()) => touch_enabled = false,
                                Err(error) => tracing::warn!("关闭触控失败，将在退出时重试：{error}"),
                            }
                        }
                        print_status(touch_enabled, inject_enabled, &options);
                    }
                    MenuCommand::Status => {
                        print_status(touch_enabled, inject_enabled, &options);
                        match connection.write(&touch_read_packet()).await {
                            Ok(()) => tracing::info!(
                                "已请求戒指真实触控状态；请查看随后 RX 中的当前休眠=是/否"
                            ),
                            Err(error) => tracing::warn!("读取戒指触控状态失败：{error}"),
                        }
                    }
                    MenuCommand::Help => print_menu(touch_enabled, inject_enabled, &options),
                    MenuCommand::Quit => {
                        tracing::info!("收到退出指令，正在释放按键并关闭触控");
                        break;
                    }
                    MenuCommand::Invalid(value) => {
                        println!("无效选择：{value}");
                        println!("请输入 1、2、3、4、5、9 或 0。");
                    }
                }
            }
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("收到 Ctrl+C，正在释放按键并关闭触控");
                break;
            }
        }
    }

    disable_host_control(&mut engine, &mut injector, &mut inject_enabled);
    if touch_enabled {
        if let Err(error) = set_touch_enabled(&connection, &options, false).await {
            tracing::warn!("关闭戒指触控模式失败：{error}");
        } else {
            tracing::info!("CONTROL_DONE 已关闭戒指触控模式");
        }
    }
    let _ = connection.disconnect().await;
    if let Err(error) = pointer_suppression.restore() {
        tracing::warn!("恢复 R08 HID 鼠标子设备失败：{error:#}");
        if run_result.is_ok() {
            run_result = Err(error.context("退出时未能恢复 R08 HID 鼠标子设备"));
        }
    }
    run_result
}

fn parse_menu_command(value: &str) -> MenuCommand {
    let normalized = value.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "1" | "listen" | "touch on" => MenuCommand::ListenOnly,
        "2" | "control" | "control on" => MenuCommand::Control,
        "3" | "pause" | "control off" => MenuCommand::PauseControl,
        "4" | "off" | "touch off" => MenuCommand::DisableTouch,
        "5" | "status" => MenuCommand::Status,
        "9" | "help" | "?" => MenuCommand::Help,
        "0" | "quit" | "exit" | "q" => MenuCommand::Quit,
        _ => MenuCommand::Invalid(value.trim().to_string()),
    }
}

fn print_menu(touch_enabled: bool, inject_enabled: bool, options: &SessionOptions) {
    println!();
    println!("========== R08 智能戒指控制 ==========");
    print_status(touch_enabled, inject_enabled, options);
    println!(
        "  1  启用智能触控（双击模式，{} 分钟后自动休眠，仅监听）",
        options.sleep_minutes
    );
    println!("  2  启用智能触控并开启电脑控制");
    println!("  3  暂停电脑控制（保留触控监听）");
    println!("  4  完全关闭智能触控（同时暂停电脑控制）");
    println!("  5  查询电脑端及戒指真实触控/休眠状态");
    println!("  9  重新显示菜单");
    println!("  0  安全退出");
    println!("======================================");
    print!("请选择：");
    let _ = io::stdout().flush();
}

fn print_status(touch_enabled: bool, inject_enabled: bool, options: &SessionOptions) {
    println!(
        "状态：智能触控策略={}（应用类型 {}，自动休眠 {} 分钟），电脑控制={}",
        if touch_enabled { "已启用" } else { "关闭" },
        options.touch_type,
        options.sleep_minutes,
        if inject_enabled { "开启" } else { "关闭" }
    );
}

async fn set_touch_enabled(
    connection: &RingConnection,
    options: &SessionOptions,
    enabled: bool,
) -> Result<()> {
    let packet = if enabled {
        touch_enable_packet(options.touch_type, options.sleep_minutes)
    } else {
        touch_disable_packet(options.sleep_minutes)
    };
    connection.write(&packet).await?;
    if enabled {
        tracing::info!(
            "智能触控策略已启用：应用类型={}，{} 分钟后自动休眠；双击唤醒由戒指固件处理",
            options.touch_type,
            options.sleep_minutes
        );
    } else {
        tracing::info!("智能触控策略已完全关闭");
    }
    Ok(())
}

fn enable_host_control(
    engine: &mut MappingEngine,
    injector: &mut Box<dyn Injector>,
    inject_enabled: &mut bool,
) -> Result<()> {
    if *inject_enabled {
        return Ok(());
    }
    let new_injector = create_injector().context("创建输入注入后端失败")?;
    *injector = new_injector;
    engine.set_inject_enabled(true);
    *inject_enabled = true;
    Ok(())
}

fn disable_host_control(
    engine: &mut MappingEngine,
    injector: &mut Box<dyn Injector>,
    inject_enabled: &mut bool,
) {
    engine.set_inject_enabled(false);
    let _ = injector.release_all();
    *injector = Box::new(NullInjector);
    *inject_enabled = false;
}

fn apply_outputs(injector: &mut Box<dyn Injector>, outputs: Vec<Output>) -> Result<()> {
    for output in outputs {
        match output {
            Output::Log(text) => tracing::info!("{text}"),
            Output::Wheel(delta) => injector.wheel(delta)?,
            Output::CaptureCursorAnchor => injector.capture_cursor_anchor()?,
            Output::RestoreCursor => injector.restore_cursor()?,
            Output::ReleaseLeftButton => injector.release_left_button()?,
            Output::Copy => injector.copy()?,
            Output::Paste => injector.paste()?,
        }
    }
    let _ = io::stdout().flush();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{parse_menu_command, MenuCommand};

    #[test]
    fn numeric_menu_commands_are_stable() {
        assert_eq!(parse_menu_command("1"), MenuCommand::ListenOnly);
        assert_eq!(parse_menu_command("2"), MenuCommand::Control);
        assert_eq!(parse_menu_command("3"), MenuCommand::PauseControl);
        assert_eq!(parse_menu_command("4"), MenuCommand::DisableTouch);
        assert_eq!(parse_menu_command("5"), MenuCommand::Status);
        assert_eq!(parse_menu_command("9"), MenuCommand::Help);
        assert_eq!(parse_menu_command("0"), MenuCommand::Quit);
    }

    #[test]
    fn textual_aliases_remain_available_for_cli_users() {
        assert_eq!(parse_menu_command("control on"), MenuCommand::Control);
        assert_eq!(parse_menu_command("touch off"), MenuCommand::DisableTouch);
        assert_eq!(parse_menu_command("exit"), MenuCommand::Quit);
    }
}
