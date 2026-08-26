# R08 固件改造路线

## 目标

目标不是继续放大现有离散手势，而是在戒指固件内提高传感器采样率，并输出适合连续滚动的事件：

- 触控采样或 IMU 采样建议至少 50 Hz；
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
- FCC ID `2AOM3-R08` 有三页公开内部照片，但芯片丝印仍需从原始清晰图片确认。
- 同协议 R02/RY02 的公开研究显示过 `0x50` 头、`e5 c3 bd 81` magic 和 payload sum32，但这些只能作为检测线索，不能假定 R08 使用相同容器或内存布局。

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
py -m unittest discover -s firmware_research\scripts -p "test_*.py"
```

检查器只读取文件并输出 JSON，不会连接戒指或写入 DFU。即使输出 `offline_patch_candidate=true`，`flash_authorized` 仍固定为 `false`；必须继续完成 MCU、签名和恢复路径验证。

## 一手资料入口

- FCC R08 设备档案：<https://fccid.io/2AOM3-R08>
- FCC R08 内部照片：<https://fccid.io/2AOM3-R08/Internal-Photos/Internal-photos-7833424>
- 同协议 R02 离线研究：<https://github.com/aimindseye/colmi-r02-firmware>
