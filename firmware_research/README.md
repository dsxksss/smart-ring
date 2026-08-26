# R08 固件研究（只读取证阶段）

## 已确认的设备身份

以下值在 Windows 上通过 BLE Device Information Service（0x180A）直接读取，未向戒指写入数据：

- 广播名称：`R08_9C07`
- BLE 地址：`31:31:45:37:9C:07`
- Hardware Revision（0x2A27）：`RT08_V3.1`
- Firmware Revision（0x2A26）：`RT08_3.10.48_260309`
- System ID（0x2A23）：`07 9C 37 00 00 45 31 31`

读取命令：

```bash
cargo run -p r08 --release -- device-info
```

Windows 遗留 C# 工具：

```powershell
.\native_ble\bin\Release\net10.0\R08NativeCli.exe --device-info
```

Rust / C# 的 `--device-info` / `device-info` 都使用只读 GATT 读取，不会执行特征写入。

## 官方 App 中确认的 OTA 机制

官方 App APK 中的 `com.oudmon.ble.base.communication.DfuHandle` 使用自定义 BLE DFU：

- Service：`de5bf728-d711-4e47-af26-65e3012a5dc7`
- Notify：`de5bf729-d711-4e47-af26-65e3012a5dc7`
- Write Without Response：`de5bf72a-d711-4e47-af26-65e3012a5dc7`
- 帧头：`0xBC`
- 命令：Start=`1`、Init=`2`、Data=`3`、Check=`4`、End=`5`
- 数据块：1024 字节，再按 ATT 长度拆包
- Init 携带固件长度、整文件 CRC16、16 位累加和
- App 接受的最大文件长度：12,288,000 字节

目前没有在 App 的协议实现或戒指暴露的 BLE 服务中找到“读取整片 Flash/导出当前固件”的命令。这个 DFU 通道是写入升级通道，不能视为备份通道。

官方 OTA 查询需要同时提交：`hardwareVersion`、`romVersion`、MAC、地区和运行/开发通道。App 下载文件到：

```text
/sdcard/Android/data/com.qcwireless.ring/files/dfu/<version>.bin
```

APK 本身不内置固件 `.bin`。

2026-08-26 使用空 token 查询上述中国区官方接口，服务器返回 `retCode=401` 和 `Not logged in yet or token has expired`。随后从当前 QRing APK 还原并实测了官方访客流程：`GET token/getToken?key=qcwx_android` 返回 `retCode=0` 和临时令牌。程序可以自行申请访客令牌，无需账号，也不提取手机私有数据；令牌不显示、不落盘。空 token 的原始记录见 `ota_query_20260826.json`。

同日通过 ADB 控制官方 App 的正常界面进行了只读检查：戒指已连接，App 显示固件 `3.10.48`；`files/dfu/` 尚不存在。访客令牌查询当前版本后，OTA 接口返回 `retCode=60001 / No upgraded version`，确认 App 的提示确实表示最新版，不是认证失败。

随后只在 OTA 查询字段中报告较低版本 `RT08_3.10.47_260101`，服务器返回精确目标 `RT08_V3.1 / RT08_3.10.48_260309`。程序以真实目标版本再次校验后下载原厂包，不连接戒指、不发送 DFU：

- 文件：`firmware_research/evidence/ota/RT08_3.10.48_260309.bin`
- 大小：`146812` 字节
- SHA-256：`c205290a7fcbc816b6be8d40f3e74d533551e0e7f2ebed9090a5d3b1c5ab613b`
- 容器：`0x50` 字节头，两个 payload 长度均为 `146732`，sum32 `13582245` 校验一致
- 架构证据：BlueX RF03 应用基址候选 `0x00824000`，189 个映射范围内的奇数 Thumb 入口指针，包含 RF03/QC 的 gsensor 与 ISR 源路径标记
- 访客令牌未显示、未写入文件；元数据记录 `authenticationStored=false`、`dfuSent=false`

工作区原件与系统临时目录研究副本的 SHA-256 一致；它们仍在同一台电脑上，不能算两个独立存储介质。

## 触控强度字段错位与离线实验候选

对 QRing APK 的 `TouchControlReq`、`RingGestureActivity` 与原厂镜像命令 `0x3B` 处理链进行逐字节对照后，确认 App 把手势强度放在数据包偏移 `4`，而固件原本把强度硬编码为 `1` 并读取偏移 `5`。这会使 App 的 `1..10` 强度选择无法按预期进入固件配置。

已生成一个只修复该字段错位的离线实验镜像：

- 文件：`firmware_research/evidence/ota/RT08_3.10.48_260309-gesture-strength-experimental.bin`
- SHA-256：`2a382a1edd756997d22a2d04a8448cf6f8e14f0deba243de44a2cd52207d20a9`
- 原厂镜像保持不变；实验镜像 `flashAuthorized=false`

完整调用链、补丁字节、验证结果与限制见 `RT08_OFFLINE_ANALYSIS_20260826.md`。该补丁只让已有强度参数生效，不等于实现连续转动滚动。

## “已备份”的判定

只有满足下列任一条件，才把固件标为已备份：

1. 从官方 App 缓存或官方 OTA 下载到与 `RT08_V3.1` 精确匹配的原厂 `.bin`，记录来源、长度和 SHA-256；或
2. 确认 MCU 型号和调试接口后，通过 SWD/JTAG/厂商烧录口读取完整 Flash，并至少重复读取两次、逐字节一致（同时确认未启用读保护）。

仅保存 APK、BLE 日志或版本号不等于备份了戒指固件。

## 防变砖硬门槛

在以下项目全部完成之前，不发送 DFU Start/Init/Data/Check/End，也不进入擦除或升级模式：

- [x] 已取得与 `RT08_V3.1` 精确匹配的原厂 `.bin`
- [ ] 已记录原厂文件 SHA-256，并做了至少两份独立副本
- [ ] 已识别 MCU/SoC、镜像架构、装载地址、向量表和分区布局
- [ ] 已判断固件是否有签名、加密、反回滚或 Secure Boot
- [ ] 已验证可操作的恢复路径（Boot ROM/量产工具/SWD/JTAG/测试点）
- [ ] 已验证恢复路径不依赖当前可运行的应用固件
- [ ] 首次修改镜像不在唯一的一枚戒指上试刷
- [ ] 升级时电量充足、连接稳定，并避免电脑睡眠和蓝牙切换

禁止事项：跨硬件版本刷写、猜测装载地址、用 DFU 写命令“探测”、在没有恢复路径时修改向量表/Bootloader、把升级中断当作退出方式。

## 下一步

2026-08-26 已完成唯一戒指的无刷机三轴实测：`A1 04 04` 会按约 0.994 Hz 同步输出 `A1 01..05`，其中 `A1 03` 的 X/Z 对转动有明显变化，但频率不足以连续跟手。完整证据和后续定位路径见 `R08_SENSOR_OBSERVABILITY_20260826.md`。

1. 继续定位 `0x1D` 离散手势的生成与 BLE 发送链，区分触控芯片事件和加速度算法事件；
2. 已区分 LIS3DH 的两组配置：活动采样窗口的 FIFO stream 路径为 `CTRL_REG1=0x37`（25 Hz），待机 INT1 动作/唤醒检测路径才使用 `0x47`（50 Hz）；已定位最多 32 样本的 FIFO 批读、RAM 环形缓冲和 `A1 01..05` 约 1 秒打包定时器，下一步量化 FIFO 窗口/缓冲写入节拍，再设计 10～20 Hz BLE 输出候选补丁；
3. 已确认加速度判定函数 `0x00831A7C` 能调用 `0x0082B408(2)` 生成真机观察到的 `02 02 00 ... 04` 通知，随后以 `3000` 重启 `gsensor_shake_flag_timer_id` 做门控/冷却；该包至少存在一条 IMU/敲击生成路径，不应只标成触控区按钮事件；
4. 识别 RF03 分区、签名策略、Boot ROM/SWD/JTAG 测试点和独立恢复路径；
5. 用户确定只使用当前唯一的一枚戒指，因此在独立恢复路径得到验证前只做离线候选镜像，不刷写实验固件。

更完整的跨电脑交接顺序见仓库根目录 `NEXT_AI_HANDOFF.md`；可迁移的二进制资料、工具、分卷恢复方式和校验值见 `research_artifacts/README.md`。

IMU 连续滚动的独立通知协议、60 秒硬超时和安全停止条件见 `RT08_IMU_ONLY_STREAM_DESIGN_20260826.md`。该文件是离线设计，不是可刷固件授权。

关键固件地址可用 `scripts/verify_rt08_imu_stream_anchors.py` 对官方镜像做只读复核。Rust 端仅加入了候选 `A2 10` 包的解码、序号和停止条件测试，尚未启用候选命令或滚轮注入。
