# R08 固件改造路线

## 目标

目标不是继续放大现有离散手势，而是在戒指固件内提高传感器采样率，并输出适合连续滚动的事件：

- 使用已经确认的 LIS3DH 活动 FIFO 配置以 25 Hz 采样，并以 10 Hz 通知最新鲜样本；只有先证明存在更高频且无副作用的数据源，才提高采样率；
- 旋转增量必须带方向、时间间隔和序号；
- 需要死区、速度上限、松手/静止停止和反向立即切换；
- 双击、三击应在固件端稳定区分，避免电脑端等待很长的聚合窗口；
- BLE 输出应是滚动增量或旋转增量，不能继续冒充鼠标相对坐标。

## 当前硬件边界

现有公开协议只确认 `A1 03` 返回三轴加速度，没有确认陀螺仪。仅有加速度计时，可以利用重力方向检测部分姿态变化，但绕重力方向的旋转不可观测，任意姿态下的精细旋转滚动无法保证。只有确认芯片包含陀螺仪，或者触控芯片能提供连续坐标，才能把“转动戒指”作为可靠的主输入。

## 已知证据

- 目标硬件：`RT08_V3.1`；当前固件：`RT08_3.10.48_260309`。
- 原厂 App 使用 `0xBC` 分帧的 OTA 流程，但没有发现读取当前 Flash 的 BLE 命令。
- `de5bf728-d711-4e47-af26-65e3012a5dc7` 在同系列设备中也用于大数据传输，不能仅凭该 UUID 判断 MCU 或 Bootloader。
- 官方 OTA 外层是 QRing `0x50` 包装，里面是 RTL8762E 格式的 1024 字节应用头：`ic_type=12`，镜像基址 `0x00826000`，可执行体起点 `0x00826400`。
- 应用头控制位为 `0x0981`：XIP、not-ready、not-obsolete 置位，encrypted 与 boot integrity-check 位未置位。按官方 1024 字节结构纠正后，真正的 SHA-256 字段在 `0x394` 且全零；此前读取的 `0x174` 位于 RSA 公钥区域，不是 SHA。
- Realtek 官方 MP 文档给出不依赖应用固件的 UART 入口：复位时拉低 `P0_3`，以 `P3_1(RX)` / `P3_0(TX)` 通信；但 RTL8762E 读回为加密数据，尚未证明可原样恢复。
- FCC ID `2AOM3-R08` 有公开内部照片，但当前镜像站点阻止自动取得原始 PDF，PCB 测试点仍需高清照片确认。
- 原厂 OTA 接收端只允许把去掉 QRing 包装的应用写入 `0x0084E000..0x00872000`；末端 `0x00872000` 已由原厂应用作为至少两个 4 KiB 页的持久化存储起点，不能把它当作可扩展代码空间。
- OTA End 确实以参数 `(image_id=0x2793, second_argument=0)` 进入 `0x00826F2A` 激活链，并经过 ROM `0x8B94/0x8B7A/0x8A5C/0x3ED1A`；应用头 `git_ver` 没有直接作为激活参数。这些地址的官方符号、OTA Bank Header 更新/选择、运行时崩溃回滚和掉电恢复仍未证明。
- 原厂 `0x00826C22` 的指令级仿真确认 `0x2790` 与 `0x2793` 走不同描述字段：OTA Header ID 使用 resolver 对象 `+0x194`，应用 ID 范围使用 `+0x60`。精确 RTL8762E SDK 结构已把应用 `+0x60` 命名为 `T_IMG_HEADER_FORMAT.git_ver`；OTA Header `+0x194`、设备 bank 状态和激活调用关系仍未证明。
- 应用包固定链接到 `image_base=0x00826000 / exe_base=0x00826400`，接收暂存区却是 `0x0084E000`，且包内没有 Bank1 重定位应用或 OTA Header。当前分类是 `SINGLE_BANK_COPY_IMAGE_CONSISTENT`；在精确 ROM/SDK 和真机读回证明前，安全模型必须假定激活会覆盖旧应用、没有运行时回滚。

## 当前离线实现

- IMU-only patch 占 292 字节，复用原厂 25 Hz FIFO 与 RAM 环形缓冲，提供 `A1 09 01/00` 启停和 10 Hz `A2 10` 通知；每个 tick 先检查连接，断连时立即停流；
- 固件端有 12 秒硬超时和生产者索引停滞急停；主机每 8 秒续期，并在 250 ms 无数据、序号跳变、校验错误或姿态无效时停止注入；
- patch SHA-256：`0aeb8f7fd8ed84e642b38dadfa578d0185fd3aee96a55554ce2798c9a0faec0a`；
- 整包候选 SHA-256：`d55458692d51ff4d21d385e61dfba34c8296934944fe3ad493f0dc07744ec1ac`；
- 候选名称强制包含 `NON_FLASHABLE`，构建报告始终返回 `flash_allowed=false`。这是一份离线审计候选，不是刷写许可。

## 进入刷写前的硬门槛

1. 从官方 App 正常会话取得与 `RT08_V3.1` 精确匹配的原厂镜像；
2. 保存两份只读副本并记录来源、长度和 SHA-256；
3. 用 `scripts/inspect_r08_image.py` 做第一轮离线检查；
4. 识别 MCU/SoC、IMU/触控芯片、Flash 布局和调试测试点；
5. 判断签名、加密、反回滚、双分区和试运行回滚策略；
6. 建立不依赖当前应用固件的恢复路径；
7. 首次刷写使用第二枚同硬件戒指，不在唯一设备上试验。

## 取得候选镜像后的检查命令

```powershell
py firmware_research\scripts\inspect_r08_image.py path\to\RT08_candidate.bin
py firmware_research\scripts\analyze_rt08_ota_path.py research_artifacts\firmware\RT08_3.10.48_260309.bin
py firmware_research\scripts\analyze_rt08_boot_activation.py research_artifacts\firmware\RT08_3.10.48_260309.bin
py firmware_research\scripts\emulate_rt08_stock_activation.py research_artifacts\firmware\RT08_3.10.48_260309.bin
py firmware_research\scripts\emulate_rt08_stock_image_info.py research_artifacts\firmware\RT08_3.10.48_260309.bin
py -m unittest discover -s firmware_research\scripts -p "test_*.py"
```

检查器只读取文件并输出 JSON，不会连接戒指或写入 DFU。即使输出 `offline_patch_candidate=true`，`flash_authorized` 仍固定为 `false`；必须继续完成 MCU、签名和恢复路径验证。

## 一手资料入口

- FCC R08 设备档案：<https://fccid.io/2AOM3-R08>
- FCC R08 内部照片：<https://fccid.io/2AOM3-R08/Internal-Photos/Internal-photos-7833424>
- 同协议 R02 离线研究：<https://github.com/aimindseye/colmi-r02-firmware>
