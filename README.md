# R08 智能戒指控制器

目标设备：`R08_9C07`（`RT08_V3.1`）。当前戒指已经安装 v7 IMU 连续流固件，Windows 主机可把戒指转动转换为连续滚轮；原有触控模式仍可用于上下滑、双击复制和三击粘贴。

## 快速使用

使用前完全退出手机 QRing App，并关闭手机蓝牙，避免抢占 BLE 连接。

Windows 连续姿态滚轮：

```text
双击 scripts\start_r08_imu_scroll.bat
```

启动后保持戒指静止约 1 秒完成零点校准，再转动戒指滚动。默认实测参数为 `gain=0.2`、`full-speed=60`。按 Enter 或 `Ctrl+C` 会发送停止命令、释放输入并退出。

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
| `r08 imu-stream` | v7 连续姿态滚轮；默认只监听，显式 `--inject` 才注入 |
| `r08 control` | 触控上下滑、复制和粘贴 |
| `r08 listen` | 监听触控通知，不注入 |
| `r08 sensor-record` | 临时采集原厂低速传感器通道；会闪 LED，不发送 DFU |
| `r08 sensor-stop` | 发送 `A1 02`，停止原始传感器与 LED |
| `r08 ota-fetch` | 从官方接口查询或下载精确匹配的原厂镜像，不连接戒指 |
| `r08 disable-touch` | 关闭官方触控模式 |

连续滚轮等价命令：

```powershell
.\target\release\r08.exe imu-stream `
  --acknowledge-unverified-candidate `
  --inject `
  --gain 0.2 `
  --full-speed 60
```

## 已验证状态

- v7 固件 SHA-256：`575d500b385f61b6cc1cf8eb9d1a55b68da4ff49a0be32800f8f91f2d8a1ff2a`。
- 固件持续输出约 9～10 Hz 的 `A2 10` IMU 通知。
- 30 秒真机注入确认双向滚动、回正停止、8 秒续期和一次 BLE 空档后的安全恢复。
- 推荐参数下滚轮峰值约 `-12..+12`；回正时实测逐步降为 `8,6,4,2,1,0`。
- 稳态超过 250 ms 无数据会先释放输入并急停，再最多恢复两次；超过上限直接退出。
- 主机仍拒绝校验错误、序号异常、零重力向量和三轴角点饱和数据。

尚未验证长时间功耗、异常掉电后的 Bootloader 搬运恢复和独立硬件恢复入口，因此不要把 v7 描述为量产固件。

## 硬件与交互边界

- 原厂自定义 GATT `0x1D` 只有离散动作：`1` 点击、`2` 下滑、`3` 上滑；没有触摸绝对坐标、压力或接触面积。
- 左右滑和长按没有稳定独立动作码，不能可靠映射光标、退格或撤销。
- 原厂 `A1 04 04` 是光学/传感器原始模式，会让 LED 闪烁；它不是触控开关。
- v7 的连续滚轮来自 LIS3DH 姿态流，不是更高分辨率的手指触摸坐标。

## 固件研究

仓库只保留我们编写的源码、补丁、测试、协议结论和哈希，不再提交官方 APK、SDK、固件镜像、反编译目录、运行日志或候选二进制。

- 当前技术状态：[HANDOFF.md](HANDOFF.md)
- 固件研究入口：[firmware_research/README.md](firmware_research/README.md)
- v7 补丁源码：`firmware_research/patches/r08_imu_stream/`
- 主机连续滚轮：`r08/src/imu_scroll.rs`
- 哈希锁定 DFU：`r08/src/sacrificial_dfu.rs` 与 `r08/src/bin/r08_sacrificial_dfu.rs`

原厂镜像 SHA-256 为 `c205290a7fcbc816b6be8d40f3e74d533551e0e7f2ebed9090a5d3b1c5ab613b`；RTL8762E SDK v1.5.0 ZIP SHA-256 为 `ef1f47b83d60aeb54edb83a34ecc9d92218965ab18ff02256773441d35f7db52`。这些第三方文件必须由使用者从可追溯来源另行取得。
