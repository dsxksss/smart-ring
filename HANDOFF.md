# R08 项目交接

更新日期：2026-08-27。接手前先阅读 `AGENTS.md`、根目录 `README.md` 和 `firmware_research/README.md`。

## 当前结果

- 目标设备：`R08_9C07`，MAC `31:31:45:37:9C:07`。
- Hardware Revision：`RT08_V3.1`。
- Device Information 仍显示原厂字符串 `RT08_3.10.48_260309`；v7 故意未改该字段。
- 戒指已经写入并激活 v7 IMU 连续流固件。
- v7 整包 SHA-256：`575d500b385f61b6cc1cf8eb9d1a55b68da4ff49a0be32800f8f91f2d8a1ff2a`。
- v7 外层版本：`RT08_3.10.51_260827`；内部版本：`1.4.6`。
- SDK 内层摘要：`0a4b55c5f9c74d02adb0cfb4aabc1d6ccd5af55238fcd4443a70ee7a6101019a`。
- `A1 09 01/00` 启停，`A2 10` 按约 9～10 Hz 输出 IMU。
- 30 秒真机注入已经确认双向滚动、回正停止、两次续期和一次短暂 BLE 空档恢复。

推荐启动：

```text
scripts\start_r08_imu_scroll.bat
```

等价参数：

```powershell
.\target\release\r08.exe imu-stream `
  --acknowledge-unverified-candidate `
  --inject --gain 0.2 --full-speed 60
```

## 主机故障安全

- 开始前保持正常姿态约 1 秒，使用 10 个样本校准零点。
- 首包最长等待 1.5 秒；进入稳态后 250 ms 无数据立即释放输入并发送停止。
- 短暂断流或固件 `STALE` 最多恢复两次；每次先急停，并等待新的 `sequence=0`。
- 8 秒续期，早于固件 12 秒硬超时。
- 每样本滚轮输出有上限；无效校验、序号异常、零向量、角点饱和均停止。
- 当前动态重力模长接受范围为 `2000..45000`，用于容纳真实快速转动。
- 所有退出路径都会尝试发送停止、释放按键/鼠标状态并断开连接。

## 已确认协议边界

- 原厂触控命令是 `0x3B`，不是 `0x2A`。
- `0x1D` 动作为：`1` 点击、`2` 下滑、`3` 上滑；它是离散事件，不是触摸坐标流。
- 左右滑、长按没有稳定独立动作码。
- `A1 04 04` 会启动约 1 Hz 的光学/传感器原始通道并点亮 LED；停止为 `A1 02`。
- v7 连续数据来自 LIS3DH FIFO 路径，不等于获得触摸表面的绝对位置。

## 固件链

原厂镜像：

- 文件版本：`RT08_3.10.48_260309`
- 长度：`146812`
- SHA-256：`c205290a7fcbc816b6be8d40f3e74d533551e0e7f2ebed9090a5d3b1c5ab613b`
- image base：`0x00826000`
- executable base：`0x00826400`
- OTA 暂存区：`0x0084E000..0x00872000`

RTL8762E SDK v1.5.0 ZIP SHA-256：`ef1f47b83d60aeb54edb83a34ecc9d92218965ab18ff02256773441d35f7db52`。`T_IMG_HEADER_FORMAT.sha256[32]` 位于内层头偏移 `0x174`；旧的 `0x394`/全零结论错误。

v4/v5 传输成功但未激活，因为没有重算内层 SDK 摘要。v6 经官方 `prepend_header.exe` 重算摘要后返回 `A1 FC`，证明自定义应用已经激活。v7 在同一正确完整性链上加入 IMU hook，并返回连续 `A2 10`。

必要实现：

- `firmware_research/patches/r08_imu_stream/`：v7 ARMv6-M 补丁源码与链接脚本。
- `firmware_research/scripts/build_rt08_imu_stream_candidate.py`：候选构建与差异约束。
- `firmware_research/scripts/finalize_rt08_candidate_with_official_sdk.py`：调用哈希锁定的官方工具重算摘要。
- `r08/src/sacrificial_dfu.rs`：只接受精确 v7 哈希和设备身份的 DFU 实现。
- `r08/src/bin/r08_sacrificial_dfu.rs`：独立危险操作入口。

仓库不保存官方 APK、SDK、原厂固件、候选二进制、反编译目录或运行日志。第三方文件必须从可追溯来源另行取得并重新核验哈希。

## 恢复限制

原厂 OTA 包只是应用恢复镜像，不是设备整片 Flash 备份。当前只证明应用仍能启动时可通过 QRing DFU 覆盖；没有验证应用启动失败、Bootloader 搬运中断、异常掉电或独立硬件恢复路径。用户明确不拆机，也不购买第二枚设备。

任何新固件必须：

1. 精确匹配 `RT08_V3.1` 和原厂镜像哈希。
2. 重新生成候选并跑完整测试，不能复用旧候选授权。
3. 给出最终二进制 SHA-256，并取得用户对该精确哈希的新授权。
4. 不把“可牺牲设备”理解为永久授权所有后续写入。

## 开始与结束调试

开始前：

1. 运行 `scripts\stop_r08_touch.bat`。
2. 确认没有遗留的 `r08.exe`；不要误杀无关进程。
3. 退出手机 QRing App 并关闭手机蓝牙。
4. 同一时间只运行一个控制器。

结束后确认控制程序已经退出。日志、抓包和构建产物均由 `.gitignore` 排除，不得提交公开仓库。

## 回归

```powershell
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo build -p r08 --release --bins
python -m pytest -q firmware_research\scripts
```

Python 测试依赖见 `firmware_research/requirements-analysis.txt`。不要从仓库根目录无筛选收集外部 SDK 或临时参考项目的测试。
