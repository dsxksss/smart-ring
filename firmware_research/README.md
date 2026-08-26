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

2026-08-26 使用空 token 查询上述中国区官方接口，服务器返回 `retCode=401` 和 `Not logged in yet or token has expired`。请求格式和服务器入口可达，但必须使用官方 App 的正常登录会话。不会使用从 APK 中提取的默认 token 绕过认证。原始记录见 `ota_query_20260826.json`。

同日通过 ADB 控制官方 App 的正常界面进行了只读检查：戒指已连接，App 显示固件 `3.10.48`；`files/dfu/` 尚不存在。点击“固件升级”会执行服务器检查，但没有显示升级确认框，也没有产生 `.bin` 缓存。由于系统顶部通知遮挡了短暂提示，仍需确认 App 返回的是“已是最新版本”还是查询失败。整个过程中没有点击升级确认，也没有向戒指发送 DFU 数据。

## “已备份”的判定

只有满足下列任一条件，才把固件标为已备份：

1. 从官方 App 缓存或官方 OTA 下载到与 `RT08_V3.1` 精确匹配的原厂 `.bin`，记录来源、长度和 SHA-256；或
2. 确认 MCU 型号和调试接口后，通过 SWD/JTAG/厂商烧录口读取完整 Flash，并至少重复读取两次、逐字节一致（同时确认未启用读保护）。

仅保存 APK、BLE 日志或版本号不等于备份了戒指固件。

## 防变砖硬门槛

在以下项目全部完成之前，不发送 DFU Start/Init/Data/Check/End，也不进入擦除或升级模式：

- [ ] 已取得与 `RT08_V3.1` 精确匹配的原厂 `.bin`
- [ ] 已记录原厂文件 SHA-256，并做了至少两份独立副本
- [ ] 已识别 MCU/SoC、镜像架构、装载地址、向量表和分区布局
- [ ] 已判断固件是否有签名、加密、反回滚或 Secure Boot
- [ ] 已验证可操作的恢复路径（Boot ROM/量产工具/SWD/JTAG/测试点）
- [ ] 已验证恢复路径不依赖当前可运行的应用固件
- [ ] 首次修改镜像不在唯一的一枚戒指上试刷
- [ ] 升级时电量充足、连接稳定，并避免电脑睡眠和蓝牙切换

禁止事项：跨硬件版本刷写、猜测装载地址、用 DFU 写命令“探测”、在没有恢复路径时修改向量表/Bootloader、把升级中断当作退出方式。

## 下一步

1. 从手机 `Android/data/com.qcwireless.ring/files/dfu/` 查找已有缓存；
2. 若无缓存，在用户明确同意向官方 OTA 服务发送设备 MAC 和版本信息后，只读查询元数据并下载精确匹配包；
3. 对 `.bin` 做熵、字符串、文件头、向量表和 CPU 架构识别；
4. 仅在镜像和恢复路径明确后，才讨论修改与刷写。

更完整的跨电脑交接顺序见仓库根目录 `HANDOFF.md`。
