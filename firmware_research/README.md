# R08 固件研究

当前状态：v8 IMU 与触控区短提示固件已经写入 `R08_9C07 / RT08_V3.1` 并真实激活。本目录只保存我们编写的源码、验证器、测试和研究结论；不保存或分发官方固件、SDK、APK、反编译目录及候选二进制。

v8 在 v7 基础上加入触控区提示灯短序列：原厂 `0x50 55 AA` 的重复计数从 25 改为 3，并预留 `1..10` 的构建参数。哈希锁定的官方 SDK 已完成内层摘要重算，finalized SHA-256 为 `4b44c8a82f227e6697e7c5dc2633db5ed478f69ca28684b19d7fb17920d08441`。2026-08-27 对该精确哈希的真机刷写成功，刷后有效 `A2 10 sequence=0` 和触控区域准确闪烁 3 次均已确认。设计和精简边界见 `RT08_CUSTOM_FIRMWARE_V8_20260827.md`。

## 精确身份与哈希

设备：

- 名称：`R08_9C07`
- MAC：`31:31:45:37:9C:07`
- Hardware Revision：`RT08_V3.1`
- Device Information 固件字符串：`RT08_3.10.48_260309`

输入材料：

- 原厂镜像长度：`146812`
- 原厂镜像 SHA-256：`c205290a7fcbc816b6be8d40f3e74d533551e0e7f2ebed9090a5d3b1c5ab613b`
- RTL8762E SDK v1.5.0 ZIP SHA-256：`ef1f47b83d60aeb54edb83a34ecc9d92218965ab18ff02256773441d35f7db52`
- 官方 `prepend_header.exe` SHA-256：`9d71cbf180afef5f7e48e0c847277addef91e344b4ceeef91296d3de0b081c22`

已安装 v7：

- 外层版本：`RT08_3.10.51_260827`
- 内部版本：`1.4.6`
- 整包 SHA-256：`575d500b385f61b6cc1cf8eb9d1a55b68da4ff49a0be32800f8f91f2d8a1ff2a`
- SDK 内层摘要：`0a4b55c5f9c74d02adb0cfb4aabc1d6ccd5af55238fcd4443a70ee7a6101019a`

Device Information 仍显示原厂固件字符串，因为 v7 没有修改该应用字段；连续 `A2 10` 才是自定义应用激活证据。

## v7 协议与真机结果

- 启动：`A1 09 01 ... AB`
- 停止：`A1 09 00 ... AA`
- 通知：16 字节 `A2 10`，包含序号、标志、三轴、FIFO 水位、丢包和样本年龄。
- LIS3DH FIFO 采样 25 Hz，通知约 9～10 Hz。
- 固件 12 秒硬超时；主机每 8 秒续期。
- 固件在断连、FIFO 无新样本和通知失败时自行停流。

30 秒真机测试已经确认：

- 双向滚轮输出；推荐参数下峰值约 `-12..+12`。
- 回正后输出逐步降为 `8,6,4,2,1,0` 并停止。
- 续期后的旧队列尾包会被丢弃，直到收到新的 `sequence=0`。
- 一次超过 250 ms 的 BLE 空档先急停，然后成功重启流。
- 快速转动的真实动态重力可超过 20000；主机可信上限现为 45000，并继续拒绝零向量和角点饱和。

## 可复现构建链

1. 从可追溯来源取得精确原厂镜像和 RTL8762E SDK ZIP，核验上面的 SHA-256。
2. 构建 ARMv6-M 补丁：

   ```powershell
   powershell -ExecutionPolicy Bypass -File firmware_research\patches\r08_imu_stream\build.ps1
   ```

3. 从精确原厂镜像生成 pre-final 候选：

   ```powershell
   python firmware_research\scripts\build_rt08_imu_stream_candidate.py `
     path\to\RT08_3.10.48_260309.bin `
     firmware_research\patches\r08_imu_stream\build\r08_imu_stream.bin `
     --bump-internal-revision --bump-outer-revision --activation-marker `
     --allow-unverified-output --output path\to\v7-prefinal.bin
   ```

   定制 v8 额外使用：

   ```powershell
   python firmware_research\scripts\build_rt08_imu_stream_candidate.py `
     path\to\RT08_3.10.48_260309.bin `
     firmware_research\patches\r08_imu_stream\build\r08_imu_stream.bin `
     --bump-internal-revision --bump-outer-revision --activation-marker `
     --touch-indicator-repeat 3 --revision-profile imu-touch-v8 `
     --allow-unverified-output --output path\to\v8-prefinal.bin
   ```

4. 用哈希锁定的官方工具重算内层摘要：

   ```powershell
   python firmware_research\scripts\finalize_rt08_candidate_with_official_sdk.py `
     path\to\v7-prefinal.bin --sdk path\to\RTL8762E_SDK_v1.5.0.zip `
     --profile imu-v7 --allow-unverified-output --output path\to\v7-final.bin
   ```

   v8 使用 `--profile imu-touch-v8`，其 pre-final、SDK、工具、内层摘要和 finalized 输出哈希均已锁定：

   ```powershell
   python firmware_research\scripts\finalize_rt08_candidate_with_official_sdk.py `
     path\to\v8-prefinal.bin --sdk path\to\RTL8762E_SDK_v1.5.0.zip `
     --profile imu-touch-v8 --allow-unverified-output `
     --output path\to\v8-final.NON_FLASHABLE.bin
   ```

5. 最终文件必须精确得到 v7 SHA-256；任何差异都视为新候选，需要重新审计和授权。

`T_IMG_HEADER_FORMAT.sha256[32]` 位于内层头偏移 `0x174`。早期把 `0x394`/全零当作摘要字段的结论已经废弃；v4/v5 未激活正是因为没有重算真实字段。

## DFU 入口

仓库只保留一个写入实现：`r08_sacrificial_dfu`。它现在被锁定到目标设备身份，以及 v8 的大小、外层版本 `RT08_3.10.52_260827`、内部版本 `1.4.7`、激活标记、触控灯三次重复指令、SDK 摘要和最终 SHA-256 `4b44c8a82f227e6697e7c5dc2633db5ed478f69ca28684b19d7fb17920d08441`。

2026-08-27 真机刷写前确认设备 `RT08_V3.1`、MAC 精确匹配、电量 100% 和官方 DFU 服务存在。手动连接使 Windows 注册精确 MAC 的 GATT 服务接口后，危险入口使用隔离的 Win32 DFU 传输；144/144 数据块、DFU CHECK 和 DFU END 全部成功。刷后基础 GATT、电量和 v8 `A2 10 sequence=0` 验证通过，`r08 touch-indicator-test` 的一次 `50 55 AA` 请求由用户肉眼确认为正确触控区域闪烁 3 次。

普通 `r08` 命令不调用 DFU。DFU 二进制还要求显式危险模式、固定确认短语和精确哈希；这些门槛不替代用户对未来新二进制的授权。

## 保留的验证代码

- `patches/r08_imu_stream/`：补丁源码、链接脚本和构建脚本。
- `build_rt08_imu_stream_candidate.py`：精确原图、地址、原字节和差异范围校验。
- `finalize_rt08_candidate_with_official_sdk.py`：SDK ZIP、官方工具、输入候选和新摘要校验。
- `verify_rt08_imu_stream_anchors.py`：原厂函数和 hook 锚点。
- `emulate_rt08_imu_stream_patch.py`：ARMv6-M 指令级仿真。
- `emulate_rt08_touch_indicator.py`：从 v8 finalized 的真实 0x50 处理入口执行并截获原厂触控灯参数，确认重复 3 次且不进入光学 LED 路径。
- `analyze_rt08_boot_activation.py`：OTA End、校验和 ready 链。
- `inspect_r08_image.py`：容器、头结构和装载地址检查。
- 对应的 `test_*.py`：防止地址、摘要、协议或 fail-closed 约束回退。

早期传感器采集、手势字段和恢复分析仍作为证据保留，但不属于 v7 日常运行路径。详细结论分别见：

- `R08_SENSOR_OBSERVABILITY_20260826.md`
- `RT08_IMU_ONLY_STREAM_DESIGN_20260826.md`
- `RT08_TIMER_AND_PATCH_EMULATION_20260826.md`
- `RTL8762E_IMAGE_AND_RECOVERY_20260826.md`
- `SOFTWARE_ONLY_RECOVERY_STRATEGY_20260827.md`

## 恢复边界

原厂镜像是应用恢复镜像，不包含 Bootloader、系统配置、配对信息、校准、OTP/eFuse 或完整持久化数据。没有整片读回、独立恢复入口、Bootloader copy 掉电矩阵或运行时崩溃回滚证明。

用户明确不拆机且不购买第二枚设备。因此当前只能依赖“应用仍能启动时通过 QRing DFU 覆盖”，不能承诺救援应用启动失败或 Bootloader 搬运中断。

## 测试

```powershell
python -m pip install -r firmware_research\requirements-analysis.txt
python -m pytest -q firmware_research\scripts
```

测试可以使用合成镜像；需要真实官方镜像或 SDK 的用例在材料不存在时应跳过，而不是从仓库下载或提交第三方文件。
