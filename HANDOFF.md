# R08 智能戒指项目交接（2026-08-26）

目标 GitHub 仓库：<https://github.com/dsxksss/smart-ring>

接手的 AI 必须先阅读仓库根目录 `AGENTS.md` 和 `NEXT_AI_HANDOFF.md`。后者是最新、可直接执行的跨电脑接手清单。

当前开发分支为 `cursor/rust-cross-platform-032b`。换电脑后执行：

```powershell
git switch cursor/rust-cross-platform-032b
powershell -ExecutionPolicy Bypass -File .\scripts\restore_research_artifacts.ps1
```

## 目标

目标设备为 `R08_9C07`。项目希望把戒指触控映射为 Windows 操作：上下滑动滚轮、双击复制、三击粘贴，并研究是否能获得更连续的触控/转动数据。后续又开始了只读固件研究，要求在任何刷写前取得可验证的原厂固件和独立恢复路径。

## 已完成

### Windows 控制程序

- 跨平台 Rust 控制器：`r08/`（Windows / Linux / macOS）
- Python BLE 调试 GUI：`smart_ring_detector.py`（Windows 遗留）
- Windows 原生 HID/GATT 控制：`native_ble/`（Windows 遗留）
- R08 动作录制、验证和 GATT CLI：`record_r08_actions.py`、`verify_r08_touch.py`、`r08_gatt_cli.py`
- 单元测试：Rust `cargo test --workspace`；Python `test_smart_ring_detector.py`
- 启动/停止脚本：`scripts/start_r08_control.*`、`scripts/stop_r08_touch.*`，以及根目录 Windows `.bat`

已确认动作映射：

- 动作 `1`：点击；快速两次可识别为双击，较慢的三次可识别为三击
- 动作 `2`：下滑
- 动作 `3`：上滑
- 左右滑、长按没有稳定独立动作码

R08 同时暴露标准 BLE HID 鼠标。触控上下滑在 HID 中表现为相对 Y 位移，而不是自定义 `6e400003` 的连续触摸坐标。原生程序截获目标戒指的 HID 相对位移并转换为 Windows 高分辨率滚轮，同时避免把普通鼠标输入当成戒指输入。

### 设备与 BLE 结论

- 名称：`R08_9C07`
- MAC：`31:31:45:37:9C:07`
- Hardware Revision：`RT08_V3.1`
- Firmware Revision：`RT08_3.10.48_260309`
- Device Information System ID：`07 9C 37 00 00 45 31 31`
- 自定义 RX/TX：Nordic-UART 风格 `6e400003` / `6e400002`
- 触控控制命令：`0x3B`
- 触控事件通知：`0x1D`
- `A1 04 04` 会开启光学/传感器原始数据并点亮 LED，不是触控模式

### 官方 OTA/DFU 研究

官方 App 使用自定义 DFU：

- Service：`de5bf728-d711-4e47-af26-65e3012a5dc7`
- Notify：`de5bf729-d711-4e47-af26-65e3012a5dc7`
- Write Without Response：`de5bf72a-d711-4e47-af26-65e3012a5dc7`
- 帧头 `0xBC`
- Start/Init/Data/Check/End：命令 `1/2/3/4/5`
- 1024 字节逻辑数据块
- Init 包含整文件长度、CRC16、16 位累加和

官方 OTA 元数据接口为：

```text
POST https://china.qcwxwire.com/qcwx/app-update/last-ota/china
```

请求需要硬件版本、固件版本、MAC、地区、渠道和官方 App 的有效登录会话。匿名只读查询得到 `401 Not logged in yet or token has expired`，记录在 `firmware_research/ota_query_20260826.json`。没有使用 APK 中的默认 token 绕过认证。

官方 App 的固件缓存路径：

```text
/sdcard/Android/data/com.qcwireless.ring/files/dfu/<version>.bin
```

2026-08-26 官方接口已确认当前版本返回 `60001 / No upgraded version`。通过只在查询字段中报告较低版本，已经取得精确匹配的官方最新版镜像。

## 当前硬性结论

已归档精确匹配的官方镜像 `RT08_3.10.48_260309.bin`，SHA-256 为 `c205290a7fcbc816b6be8d40f3e74d533551e0e7f2ebed9090a5d3b1c5ab613b`。这不是设备整片 Flash 读回，尚未验证独立恢复入口。没有向戒指发送 DFU Start/Init/Data/Check/End，也没有执行擦除或刷写。

官方 DFU 实现没有发现读取整片 Flash 的命令；获得当前镜像只能依赖精确匹配的官方 OTA 包，或者识别芯片和测试点后通过 SWD/JTAG/厂商 Boot ROM 重复读取。

原厂包的 image base candidate 为 `0x00826000`，`exe_base/load_base` 为 `0x00826400`，但接收暂存区是 `0x0084E000..0x00872000`；包内没有 Bank1 重定位应用或独立 OTA Header。2026-08-27 取得的 RTL8762E SDK v1.5.0（ZIP SHA-256 `ef1f47b83d60aeb54edb83a34ecc9d92218965ab18ff02256773441d35f7db52`）又证明禁用 bank-switch 的官方模型是 Bank1 大小为零、OTA TMP 置 ready、复位后交给 bootloader copy。当前分类提升为 `SINGLE_BANK_COPY_IMAGE_PROVEN_AT_SDK_ARCHITECTURE_AND_R08_APPLICATION_PATH`；R08 bootloader 实际复制与掉电恢复仍未实测，安全模型必须假定激活覆盖旧应用而无运行时回滚。

2026-08-27 离线 IMU 候选已完成 ARMv6-M Thumb 指令级仿真和完整回归：57 个 Python 测试、59 个 Rust 库测试、1 个 Rust 主程序测试，Release 构建成功。原厂 FIFO 消费链、callback 自停路径和通知连接检查已有精确字节锚点；当前 292 字节补丁会在断连后、读取 FIFO 和通知前立即停流。原厂激活函数通过 5 个指令级门控/地址场景；原厂 image-info 函数通过 7 个场景。SDK 官方 ROM 符号已精确命名 `0x8AE2 get_header_addr_by_img_id`、`0x8B94 get_temp_ota_bank_addr_by_img_id`、`0x8B7A is_ota_support_bank_switch`、`0x8A5C check_image_chksum`、`0x3ED1A dfu_set_ready`；OTA `+0x194` 是 `T_OTA_HEADER_FORMAT.ver_val`，应用 `+0x60` 是 `git_ver.ver_info.version`。`scripts/verify_rtl8762e_sdk_v1_5_0.py` 直接验证 ZIP 的 10 个关键成员和内嵌 BeeMPTool 的 7 个成员，不执行厂商程序。BeeMPTool 文档/导出表确认 P0_3 复位 trap、P3_0/P3_1 UART、Flash/eFuse 读取路径及 RTL8762E 目标端代理；但工具没有公开稳定 CLI，EXE/DLL 未签名，目标测试点、安全等级和读回明文/密文语义均未确认。OTA 手册确认由 boot program 从 OTA Temp 搬到 Bank0，但未承诺掉电恢复；安全手册确认 SWD 在安全等级 1-3 被关闭，R08 当前等级未读取。候选继续强制标记 `NON_FLASHABLE`：R08 安装 Header/实际 Flash Map、bootloader copy 掉电矩阵、独立恢复及第二枚设备测试仍未完成。MP Tool 的任何读回和 `Backup files` 均未证明可回写恢复，不能算设备完整备份；`verify_r08_readback_pair.py` 只验证两次离线读回的声明范围、长度、哈希和逐字节一致性，固定拒绝把一致读回升级为恢复或刷写授权。`audit_r08_flash_readiness.py` 按 12 个证据门禁拒绝空声明、错哈希、唯一设备破坏性测试、同一次冷启动双读和同一物理介质双备份；即使技术门禁全过也固定不授权刷写。

## 明天优先继续

精确步骤和当前逆向地址见 `NEXT_AI_HANDOFF.md`；以下旧清单只保留背景。

1. 在有戒指的电脑上克隆本仓库，先跑 Rust 测试和 Release 构建：

   ```bash
   cargo test --workspace
   cargo build -p r08 --release
   cargo run -p r08 --release -- self-check
   ```

   Windows 若仍使用遗留工具：

   ```powershell
   py -m pip install -r requirements.txt
   py -m pytest -q
   dotnet build native_ble\R08NativeCli.csproj -c Release
   ```

2. 推荐用 `r08 interactive` 的数字菜单：先选 `1` 确认 GATT 动作，再选 `2` 开启电脑控制；`4` 关闭触控，`0` 安全退出。Linux 需要 `input` 组与 `/dev/uinput`；macOS 需要辅助功能权限，且精细相对 Y 不可用。
3. 直接使用 `research_artifacts/firmware/` 中已校验的官方镜像做离线识别，不再重复抓 OTA。
4. 定位 LIS3DH ODR、读取循环、`A1 01..05` 打包定时器和 `0x1D` 手势生成链。
5. 拆机或探测测试点前先拍摄高清 PCB 双面照片，确认 MCU/SoC 型号、供电电压和 SWD/JTAG/串口焊盘；不要盲接 5V。
6. 只有 `firmware_research/README.md` 的全部防变砖门槛满足后，才讨论修改或刷写。

## 明确禁止

- 不跨硬件版本刷包
- 不用 DFU 写命令做“探测”
- 不把升级中断当作退出方式
- 不在唯一一枚戒指上试刷未验证镜像
- 不上传手机 bugreport、HCI 全量日志、通知截图、配对码或账号 token

## 仓库资料包

用户已明确要求把继续研究所需资料带回家。`research_artifacts/` 包含官方固件、APK 分卷、完整反编译目录分卷、脱敏 R08 传感器证据、工具和参考项目；恢复方法及 SHA-256 见其 `README.md`。仓库不包含 ADB 配对码、账号令牌、个人通知、完整手机系统日志或附近无关 BLE 设备数据。
