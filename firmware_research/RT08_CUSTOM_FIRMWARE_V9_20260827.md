# RT08 自定义固件 v9：保留触控并屏蔽 HID 鼠标报告

日期：2026-08-27

## 目标与当前边界

v9 只解决组合控制中的一个明确冲突：开启原厂电容触控后，戒指会通过 HID 属性索引 `4` 发送鼠标报告，可能移动电脑光标。v9 在固件端让三个已确认的鼠标报告辅助函数立即返回，同时保留属性索引 `0x18` 的键盘辅助函数、v7 连续 IMU 流和 v8 的三次触控区提示。

当前戒指仍运行 v8。v9 已完成构建、官方 SDK 内层摘要重算和离线指令执行验证，但尚未取得该精确哈希的刷写授权，也没有真机验收。不要把本文件中的离线结论描述成真机结论。

## 精确候选

- 外层版本：`RT08_3.10.53_260827`
- 内部版本：`1.4.8`（原始值 `0x00008041`）
- 文件长度：`146812`
- pre-final SHA-256：`b194b95f808bef0814a3b046e3c738d42ecd3d10819576ebe823a380faf5d301`
- 官方 SDK 内层摘要：`1bc598209c53b748218d65346a20b823ee3db0f0e797ee1bfbb92db6d6570126`
- finalized SHA-256：`681dbb3e7a9112fc85b1d8e546717eb5052ae7a7138b117b6dfff75de7eba1f5`
- DFU CRC16：`0xD716`
- DFU sum16：`0xAF66`
- DFU 数据块：`144`
- 独立能力标记：校验正确的 `A1 FC`

候选二进制位于被 `.gitignore` 排除的本地 evidence 目录，不进入公开仓库。仓库只记录我们编写的构建器、验证器、测试和精确哈希。

## 鼠标报告边界

`verify_rt08_hid_mouse_anchors.py` 对原厂镜像中的完整函数字节和调用目标做哈希锁定。三个辅助函数均把 HID 属性索引 `4` 传给真正的 `server_send_data`（`0x0083D7B2`）：

- `0x00829F74`
- `0x00829FAA`
- `0x00829FD4`

v9 仅把这三个入口的前两个原字节 `1F B5` 改为 `70 47`（`bx lr`）。相邻两个属性索引 `0x18` 的键盘辅助函数 `0x0082A022`、`0x0082A04C` 保持逐字节不变。Bootloader、系统区和其他 HID 路径均不修改。

`emulate_rt08_hid_mouse_block.py` 从 finalized 镜像真实入口执行三个函数，确认每条路径只经过入口和返回哨兵，没有到达 `server_send_data`。这证明的是三个已识别入口的指令级行为，不等于已经穷举芯片内所有潜在输入报告路径。

## 保留功能

- v7 `A1 09` 启停、约 9～10 Hz 的 `A2 10` IMU 流和 12 秒固件硬超时保持不变。
- v8 `50 55 AA` 触控区提示重复 3 次保持不变。
- 原厂 `0x3B` 触控配置与 `0x1D` 离散点击/上下滑通知保持不变。
- A1 查询的独立返回标记由 v8 的 `0xFD` 改为 v9 的 `0xFC`，供主机严格区分能力。

## 主机组合模式

主机只有在同时满足以下条件时才启用 v9 原生触控组合：

1. 电量查询证明 UART 通知通道可双向收发。
2. 能力查询返回校验正确的 `A1 FC`。
3. 后端是已验证的 Windows Win32 GATT 路径。
4. `3B 02 00 02 01` 收到精确应答。

满足后，待机不再用 IMU 冲击猜测双击，只接受电容触控通知：两个真实 `0x1D/1` 点击或设备的 `73 2A 00` 唤醒。唤醒用的双击不会被当成复制；控制窗口内双击复制、三击粘贴，姿态流负责连续上下滚轮。退出或失败路径会停止 IMU、关闭触控并释放所有主机输入状态。

未检测到 v9 时继续使用 v8 的主机 IMU 双敲兜底，并保留精确的 Windows R08 HID 子设备停用保护。

## 可复现构建

```powershell
python firmware_research\scripts\build_rt08_imu_stream_candidate.py `
  path\to\RT08_3.10.48_260309.bin `
  firmware_research\patches\r08_imu_stream\build\r08_imu_stream.bin `
  --bump-internal-revision --bump-outer-revision --activation-marker `
  --touch-indicator-repeat 3 --block-hid-mouse-reports `
  --revision-profile imu-touch-v9 --allow-unverified-output `
  --output path\to\v9-prefinal.NON_FLASHABLE.bin

python firmware_research\scripts\finalize_rt08_candidate_with_official_sdk.py `
  path\to\v9-prefinal.NON_FLASHABLE.bin `
  --sdk path\to\RTL8762E_SDK_v1.5.0.zip `
  --profile imu-touch-v9 --allow-unverified-output `
  --output path\to\v9-final.NON_FLASHABLE.bin
```

最终文件必须精确得到本文件记录的 finalized SHA-256。任何字节变化都是新候选，必须重新审计并取得新授权。

## 刷写与验收门槛

`r08_sacrificial_dfu` 已锁定 v9 的身份、大小、版本、内层摘要、外层校验、能力标记、三个鼠标入口、三次触控提示和最终 SHA-256。锁定不构成刷写授权。

刷写前必须由用户明确授权以下精确哈希：

```text
681dbb3e7a9112fc85b1d8e546717eb5052ae7a7138b117b6dfff75de7eba1f5
```

真机验收顺序：先验证 GATT、电量、`A1 FC`、`3B` 应答和 `A2 10`，且不启用 `--inject`；再由用户确认触控双击能唤醒且光标不移动；最后才显式启用注入，验证转动滚动、双击复制、三击粘贴、60 秒回待机和异常退出释放。
