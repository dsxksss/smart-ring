# R08 智能戒指控制器

目标设备：`R08_9C07`（`RT08_V3.1`）。当前戒指已经安装并激活 v11：只在原厂队列标记接触且 `abs(Y)>=16` 时输出，两套竖滑轨迹离线均固定为同向 2 格。144/144、DFU CHECK、DFU END 和重启后的 `A1 FA` 已成功；真实滑动手感仍待用户验收。此前 v10 已确认方向正确、无光标移动和无点击，但会把前导校准和尾部样本变成滚轮，造成一次滑动偶发反向且偏快。

## 快速使用

使用前完全退出手机 QRing App，并关闭手机蓝牙，避免抢占 BLE 连接。

当前默认先做最小连接验证，不启动滚轮或触控映射：

```powershell
cargo run -p r08 --release --bin r08 -- connect-check
```

程序通过只读电量查询及通知应答验证 GATT 双向链路，然后断开。它不会启动提示灯、心率/血氧光学灯、IMU、触控映射或输入注入，也不会刷写固件。当前 v8 已把 QRing 原厂“查找设备”命令 `50 55 AA` 的触控区提示从约 20 次缩短为 3 次，并经真机确认；默认连接检查仍不会主动点灯。

Windows 组合控制（推荐）：

```text
双击 scripts\start_r08_imu_scroll.bat
```

组合模式先读取独立的 A1 能力标记。v9/v10/v11 分别返回 `0xFC/0xFB/0xFA`，程序要求已验证的 Windows Win32 GATT 后端并发送官方 `3B 02 00 02 01`；电容双击可在休眠后亮绿灯并唤醒。v10/v11 的上下滑由固件生成只含滚轮的 HID 报告，必须显式传入 `--inject` 才会开启触控。旧版 `0xFD` 仍保留主机 IMU 双敲兜底。默认姿态参数为 `gain=0.2`、`full-speed=60`。按 Enter 或 `Ctrl+C` 会先停止 IMU、关闭触控并释放输入。

启动脚本仍会申请管理员权限，供 v8/旧固件兜底时精确停用 R08 HID 鼠标子设备；普通鼠标不会被停用。v9 通过固件入口短路 HID 属性索引 4 的鼠标报告；v10/v11 在同一属性上只允许滚轮字段，X/Y 和按钮恒为零。主机检测到 `A1 FC/FB/FA` 后不再停用 Windows 设备。脚本每次都会增量构建当前分支，避免误用旧二进制。

如果 Windows 蓝牙栈偶发把写入报告为“操作已被用户取消（`0x800704C7`）”，并不表示用户真的点了取消。程序只会对这个瞬时错误安全重试同一幂等数据包；其他错误仍会直接停止。

IMU 启动前会先发送官方只读 UART 电量查询并等待应答。出现 `UART_NOTIFY_READY` 说明通知订阅确实可用；只有通过该握手后才发送 `A1 09`。IMU 启停和续期使用 BLE 无应答写入，并继续以首包、连续序号和固件 12 秒硬超时验证结果，避免 Windows WinRT 在频繁等待写入应答时关闭 GATT 对象。若握手成功但始终没有 v7 的 `A1 FE`/`A2 10`，应优先检查当前运行应用是否仍为已激活的 v7，而不是继续调整 Windows 超时。

原有触控控制（上下滑滚轮、双击复制、三击粘贴）：

```text
双击 scripts\start_r08_control.bat
```

停止或清理残留触控状态：

```text
双击 scripts\stop_r08_touch.bat
```

## 构建与验证

需要 Rust 1.85 或更新版本：

```powershell
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo build -p r08 --release --bins
```

常用命令：

| 命令 | 作用 |
| --- | --- |
| `r08 self-check` | 检查本机 BLE、HID 和输入注入后端，不连接戒指 |
| `r08 scan` | 扫描目标戒指 |
| `r08 device-info` | 只读设备身份 |
| `r08 imu-stream` | 连续姿态滚轮；加 `--double-tap-wake` 后 v9-v11 使用电容触控双击，v8 使用 IMU 双敲兜底 |
| `r08 control` | 触控上下滑、复制和粘贴 |
| `r08 listen` | 监听触控通知，不注入 |
| `r08 sensor-record` | 临时采集原厂低速传感器通道；会闪 LED，不发送 DFU |
| `r08 sensor-stop` | 发送 `A1 02`，停止原始传感器与 LED |
| `r08 touch-raw` | 只读观察触控控制器 C1～C4；不启用光学原始模式，不注入输入 |
| `r08 touch-indicator-test` | 发送一次已知的 `50 55 AA` 触控区提示命令；v8 真机确认为 3 次 |
| `r08 ota-fetch` | 从官方接口查询或下载精确匹配的原厂镜像，不连接戒指 |
| `r08 disable-touch` | 关闭官方触控模式 |

连续滚轮等价命令：

```powershell
.\target\release\r08.exe imu-stream `
  --acknowledge-unverified-candidate `
  --inject `
  --double-tap-wake `
  --touch-scroll-only `
  --seconds 0 `
  --gain 0.2 `
  --full-speed 60
```

省略 `--double-tap-wake` 时保留原有的立即启动 IMU 行为；`--seconds` 仍表示程序总运行时长，`0` 表示持续待机直到按 Enter 或 `Ctrl+C`。

## 已验证状态

- 当前真机安装 v11 SHA-256：`7b60058f5d4de8246834acf139b059009495e0dc9a811b5ff041ec33e3e00e0f`；大小 `146812`、CRC16 `0x0349`、sum16 `0xAE68`、144 块，SDK 内层摘要 `be0ced2e6d3d05b9b4080fb84a29f698b11aff4357a2efa138c745bb606c660a`。已完成写入并返回 `A1 FA`，真实触控滚动验收待完成。
- v10 SHA-256：`6cd256de135ce4290794feebec808cdf4cea2e6fd9dfdd30e675a16fcb7927bb`；触控滚动、方向正确、无光标移动和无点击已确认。已知问题是逐帧 Y 符号转换会造成偶发反向和偏快。
- v11 指令级仿真确认两套原厂竖滑数组分别只输出 `[-1,-1]` 和 `[+1,+1]`，前导校准、小幅运动、松开和尾部样本均为 0；按钮、X、Y 恒为 0。当前应用接口不含上下电极权重或静止按住坐标，因此不能把这一版描述成真实上下半区 hold-to-scroll。
- 下一阶段目标是按住触控区上半/下半持续慢速滚动并在松手时立即停止；这需要继续逆向触控控制器原始接触/区域接口，v11 的离散两格滑动不等同于该功能。
- 已定位原厂触控控制器四路只读诊断值：一次性 `A1 03` 查询会返回 `A1 04`，其中 C1～C4 是寄存器 `0x61/0x65/0x69/0x6D` 的四个 16 位值。`r08 touch-raw` 可观察这些值；它不发送会开启光学原始模式的 `A1 04 04`，也不注入输入。四通道与触控区上下位置的物理映射仍须真机采样确认，不能凭编号猜测。
- v7 固件 SHA-256：`575d500b385f61b6cc1cf8eb9d1a55b68da4ff49a0be32800f8f91f2d8a1ff2a`。
- 固件持续输出约 9～10 Hz 的 `A2 10` IMU 通知。
- 30 秒真机注入确认双向滚动、回正停止、8 秒续期和一次 BLE 空档后的安全恢复。
- 2026-08-27 已确认 v8/旧路径发送 `3B 02 00 02 01` 后 WinRT GATT 对象会关闭，因此旧固件仍禁止组合发送。v9 已证明 `A1 FC + Windows Win32 GATT + 3B` 可稳定双击唤醒，但触控滑动的原厂输出是 HID 鼠标运动而非 GATT 动作；v10 针对此真机结论改写为安全滚轮。
- 推荐参数下滚轮峰值约 `-12..+12`；回正时实测逐步降为 `8,6,4,2,1,0`。
- 稳态超过 750 ms 无数据会立即停止主机输入，并重发幂等的 IMU 启动命令；不在每次短空档额外发送停止命令，避免固件 timer/FIFO 被反复拆建。恢复后的首包超时使用同一个快速恢复预算，连续失败先快速恢复两次；组合控制器耗尽预算后改为 1 秒退避并持续重试，不退出待机程序。Windows WinRT 已实测存在超过 250 ms 的通知抖动，而滚轮完全由新样本驱动、没有主机惯性，因此等待和重启期间不会继续滚动。重启后连续收到 10 个有效样本会清零快速恢复预算。
- 主机仍拒绝校验错误、序号异常、零重力向量和三轴角点饱和数据。

此前约 60 秒后出现的 `73 2A 01` 与未触摸时的自动休眠时刻一致，因此状态含义为 `00=唤醒`、`01=休眠`。v9 真机已确认休眠后双击触控区会亮绿灯并产生 `73 2A 00`；滑动却不会产生 `0x1D/2/3`。

尚未验证长时间功耗、异常掉电后的 Bootloader 搬运恢复和独立硬件恢复入口，因此不要把 v7 描述为量产固件。

## 硬件与交互边界

- 协议解析支持原厂 GATT `0x1D` 离散动作 `1/2/3`，但 v9 真机的应用类型 2 上下滑没有发出 `0x1D/2/3`；其实际输出来自 HID 鼠标运动队列。
- 左右滑和长按没有稳定独立动作码，不能可靠映射光标、退格或撤销。
- 原厂 `A1 04 04` 是光学/传感器原始模式，会让 LED 闪烁；它不是触控开关。
- 触控四通道只读观察：`cargo run -p r08 --release --bin r08 -- touch-raw --seconds 30 --interval-ms 500`。运行时依次保持未触摸、按住触控区上半、按住触控区下半，并比较 C1～C4；该命令不会控制滚轮。
- v7 的连续滚轮来自 LIS3DH 姿态流，不是更高分辨率的手指触摸坐标。

## 固件研究

仓库只保留我们编写的源码、补丁、测试、协议结论和哈希，不再提交官方 APK、SDK、固件镜像、反编译目录、运行日志或候选二进制。

- 当前技术状态：[HANDOFF.md](HANDOFF.md)
- 固件研究入口：[firmware_research/README.md](firmware_research/README.md)
- v7-v11 共用的 IMU 补丁源码：`firmware_research/patches/r08_imu_stream/`
- v9 设计与精确哈希：`firmware_research/RT08_CUSTOM_FIRMWARE_V9_20260827.md`
- v10 触控滚轮改写与精确哈希：`firmware_research/RT08_CUSTOM_FIRMWARE_V10_20260828.md`
- v10 30 字节滚轮补丁源码：`firmware_research/patches/r08_touch_wheel/`
- v11 接触门控低速滚轮与精确哈希：`firmware_research/RT08_CUSTOM_FIRMWARE_V11_20260828.md`
- 触控控制器四路只读路径：`firmware_research/RT08_TOUCH_RAW_CHANNELS_20260828.md`
- v11 42 字节补丁源码：`firmware_research/patches/r08_touch_wheel_v11/`
- 主机连续滚轮：`r08/src/imu_scroll.rs`
- 哈希锁定 DFU：`r08/src/sacrificial_dfu.rs` 与 `r08/src/bin/r08_sacrificial_dfu.rs`

原厂镜像 SHA-256 为 `c205290a7fcbc816b6be8d40f3e74d533551e0e7f2ebed9090a5d3b1c5ab613b`；RTL8762E SDK v1.5.0 ZIP SHA-256 为 `ef1f47b83d60aeb54edb83a34ecc9d92218965ab18ff02256773441d35f7db52`。这些第三方文件必须由使用者从可追溯来源另行取得。
