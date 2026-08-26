use std::io::{self, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc as tokio_mpsc;

use crate::ble::RingConnection;
use crate::mapping::{InputEvent, MappingConfig, MappingEngine, Output};
use crate::platform::inject::{Injector, NullInjector};
use crate::platform::{create_injector, spawn_hid_monitor};
use crate::protocol::{touch_disable_packet, touch_enable_packet};

pub struct SessionOptions {
    pub inject: bool,
    pub touch_type: u8,
    pub sleep_minutes: u8,
    pub scroll_gain: i32,
    pub seconds: u64,
}

pub async fn run(connection: RingConnection, options: SessionOptions) -> Result<()> {
    let mut notifications = Box::pin(connection.subscribe().await?);
    connection
        .write(&touch_enable_packet(
            options.touch_type,
            options.sleep_minutes,
        ))
        .await?;

    let mut engine = MappingEngine::new(MappingConfig {
        scroll_gain: options.scroll_gain.clamp(1, 10),
        inject: options.inject,
    });
    let mut injector: Box<dyn Injector> = if options.inject {
        create_injector().context("创建输入注入后端失败")?
    } else {
        Box::new(NullInjector)
    };

    let (hid_tx, hid_rx) = mpsc::channel();
    let _hid = spawn_hid_monitor(hid_tx).context("启动 HID 监听失败")?;
    if options.inject {
        tracing::info!(
            "CONTROL_READY 类型={}；滚轮增益={}；双击=复制，三击=粘贴，按 Enter 退出",
            options.touch_type,
            options.scroll_gain
        );
    } else {
        tracing::info!(
            "LISTEN_READY 类型={}；仅记录动作，不注入；按 Enter 退出",
            options.touch_type
        );
    }

    let (stop_tx, mut stop_rx) = tokio_mpsc::channel::<()>(1);
    tokio::spawn(async move {
        let mut stdin = BufReader::new(tokio::io::stdin());
        let mut line = String::new();
        let _ = stdin.read_line(&mut line).await;
        let _ = stop_tx.send(()).await;
    });

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
            _ = stop_rx.recv() => {
                tracing::info!("收到退出指令");
                break;
            }
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("收到 Ctrl+C，正在释放按键并关闭触控");
                break;
            }
        }
    }

    let _ = injector.release_all();
    if let Err(error) = connection
        .write(&touch_disable_packet(options.sleep_minutes))
        .await
    {
        tracing::warn!("关闭戒指触控模式失败：{error}");
    } else {
        tracing::info!("CONTROL_DONE 已关闭戒指触控模式");
    }
    let _ = connection.disconnect().await;
    run_result
}

fn apply_outputs(injector: &mut Box<dyn Injector>, outputs: Vec<Output>) -> Result<()> {
    for output in outputs {
        match output {
            Output::Log(text) => tracing::info!("{text}"),
            Output::Wheel(delta) => injector.wheel(delta)?,
            Output::RestoreCursor => injector.restore_cursor()?,
            Output::ReleaseLeftButton => injector.release_left_button()?,
            Output::Copy => injector.copy()?,
            Output::Paste => injector.paste()?,
        }
    }
    let _ = io::stdout().flush();
    Ok(())
}
