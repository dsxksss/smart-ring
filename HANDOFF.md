# R08 智能戒指项目交接（2026-08-26）

目标 GitHub 仓库：<https://github.com/dsxksss/smart-ring>

接手的 AI 必须先阅读仓库根目录 `AGENTS.md`。其中集中记录了安全红线、蓝牙互斥、残留进程处理、滚轮数据上限和禁止上传的敏感内容。

本地仓库已经提交到 `main`。如果远程仍为空，在本目录执行：

```powershell
git push -u origin main
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

2026-08-26 使用官方 App 正常登录/连接后检查：外部目录中只有设备指南图片，未出现 `dfu` 目录或固件文件。点击“固件升级”后 App 显示短暂提示，没有进入升级确认框，也没有下载缓存；提示被系统通知遮挡，尚未确认是“已是最新版本”还是查询失败。

## 当前硬性结论

**戒指当前固件尚未备份。** 已保存 APK、版本号、协议和日志都不等于备份。没有向戒指发送 DFU Start/Init/Data/Check/End，也没有执行擦除或刷写。

官方 DFU 实现没有发现读取整片 Flash 的命令；获得当前镜像只能依赖精确匹配的官方 OTA 包，或者识别芯片和测试点后通过 SWD/JTAG/厂商 Boot ROM 重复读取。

## 明天优先继续

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

2. 用 `r08 listen` 确认 GATT 动作后再开 `r08 control`。Linux 需要 `input` 组与 `/dev/uinput`；macOS 需要辅助功能权限，且精细相对 Y 不可用。
3. 手机安装/登录官方 App，连接戒指，进入“我的 → 固件升级”。只允许服务器检查和下载；**不要确认升级**。
4. 检查 `/sdcard/Android/data/com.qcwireless.ring/files/dfu/`。如出现 `.bin`，先原样拉取、只读保存两份，并记录来源、长度和 SHA-256。
5. 如果 App 明确显示“已是最新版本”且不缓存当前包，联系厂商索取 `RT08_V3.1 / RT08_3.10.48_260309` 原厂包，或在授权范围内记录 App 正常会话的 OTA 响应。不要使用硬编码/default token。
6. 取得镜像后只做离线识别：文件头、熵、字符串、ARM 向量表、装载地址、分区、签名/加密/反回滚。
7. 拆机或探测测试点前先拍摄高清 PCB 双面照片，确认 MCU/SoC 型号、供电电压和 SWD/JTAG/串口焊盘；不要盲接 5V。
8. 只有 `firmware_research/README.md` 的全部防变砖门槛满足后，才讨论修改或刷写。

## 明确禁止

- 不跨硬件版本刷包
- 不用 DFU 写命令做“探测”
- 不把升级中断当作退出方式
- 不在唯一一枚戒指上试刷未验证镜像
- 不上传手机 bugreport、HCI 全量日志、通知截图、配对码、账号 token 或官方 APK

## 本仓库未包含的本地证据

为了隐私与版权，GitHub 不包含官方 APK、反编译源码、Android bugreport、HCI 抓包、ADB 工具、个人通知截图和本机日志。需要复核时，应从官方渠道重新下载 App，并在自己的设备上重新抓取。公开的结论和必要哈希写在文档中。
