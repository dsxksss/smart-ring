# 下一位 AI 接手清单（2026-08-26）

## 先读这些文件

依次完整阅读：`AGENTS.md`、本文件、`README.md`、`firmware_research/README.md`、`firmware_research/R08_SENSOR_OBSERVABILITY_20260826.md` 和 `firmware_research/RT08_OFFLINE_ANALYSIS_20260826.md`。不要重复已经完成的 APK、OTA 和协议逆向。

工作分支为 `cursor/rust-cross-platform-032b`。用户回家后只使用当前这一枚 R08 戒指，不会购买第二枚测试设备。

## 当前可复核事实

- 设备：`R08_9C07`，`31:31:45:37:9C:07`。
- 硬件/固件：`RT08_V3.1 / RT08_3.10.48_260309`。
- 官方镜像已通过 QRing 访客 OTA 流程取得，长度 `146812`，SHA-256 `c205290a7fcbc816b6be8d40f3e74d533551e0e7f2ebed9090a5d3b1c5ab613b`。
- 官方镜像不是从设备整片读回；尚未确认 Flash 分区、签名/反回滚，也没有验证独立恢复入口。
- 离线手势强度字段补丁已生成，但 `flashAuthorized=false`，绝对不能把它当成可刷版本。
- `A1 04 04` 实测 20.125 秒收到 21 个 `A1 03` 样本，有效频率约 `0.994 Hz`；X 范围 996、Y 范围 196、Z 范围 908。开始和 `A1 02` 停止均成功，没有发送 DFU。
- `A1 01..05` 同步约 1 Hz，说明限制更可能来自上层打包定时器，尚不能断定 LIS3DH 本身只工作在 1 Hz。
- 固件字符串附近的 Thumb 回调候选：`gsensor_read_timer_id -> 0x00833A0B`，`gsensor_shake_flag_timer_id -> 0x00833A63`。没有普通 `BL` 直接调用者，只能作为函数指针或定时器注册候选继续验证。
- 现有证据只支持用重力向量估计俯仰或横滚；没有陀螺仪证据，绕重力轴的纯旋转不可观测，不能承诺完整“拧戒指”控制。
- ATC_RF03_Ring 使用 STK8321，目标 R08 指向 LIS3DH。它只能参考 BlueX RF03 研究方法和 OTA 容器，不能直接刷固件或复用驱动。

## 回家后的环境恢复

```powershell
git switch cursor/rust-cross-platform-032b
powershell -ExecutionPolicy Bypass -File .\scripts\restore_research_artifacts.ps1
cargo test --workspace
cargo build -p r08 --release
```

需要连接戒指时，先完全退出手机 QRing 并关闭手机蓝牙；把戒指放回充电器再取出唤醒。Windows 上 BLE 缓存或旧 HID 配对可能造成“能看见但连不上”，先停止本项目残留控制进程，再处理精确匹配的 R08 设备，不能误停普通键盘或鼠标。传感器采集在完整管理员桌面会话中成功过。

资料包说明、分卷恢复和 SHA-256 见 `research_artifacts/README.md`。其中没有 ADB 配对码、登录令牌或附近无关 BLE 扫描数据。

## 下一阶段准确任务

1. 沿 `A1 04 04` 命令处理器定位 `A1 01..05` 的数据包构造与通知发送函数。
2. 从 `qc_code\\app_module\\gsensor\\lis3dh_spi.c` 字符串交叉引用定位 LIS3DH 初始化、`CTRL_REG1`/ODR、SPI 读取和姿态滤波调用链。
3. 沿两个 timer ID 和相邻 Thumb 指针定位注册结构、间接调用和周期参数，证明当前约 1 秒周期的真正来源，不能直接猜改立即数。
4. 离线设计“50 Hz 内部采样、10～20 Hz BLE 批量输出”的候选方案，并让加速度采样脱离会点亮绿灯的光学测量状态机。
5. 手势状态机目标：双击唤醒高频模式，60 秒后回到低功耗；上下、俯仰或横滚控制滚轮，保留明确死区、速度上限和松手停止。先用记录数据验证算法，再讨论固件实现。
6. 每个二进制补丁必须同时校验原镜像 SHA-256、精确地址、原始指令字节、Thumb 控制流和 RF03 容器 sum32；输出差异清单，不连接戒指，不调用 DFU。
7. 在唯一戒指刷写前，识别 MCU/SoC、Boot ROM、量产接口或 SWD 测试点，并证明恢复路径不依赖当前应用固件。没有独立恢复能力就继续离线研究，不刷实验镜像。

## 仍未完成，不能写成已完成

- 没有把任何修改固件刷入戒指。
- 没有从戒指整片读回 Flash，也没有双介质恢复备份。
- 没有确认 MCU 型号、分区、Secure Boot、签名或反回滚。
- 没有验证 SWD、JTAG 或 Boot ROM 恢复。
- 没有找到 1 Hz 定时器的确切周期立即数。
- 没有实现或真机验证 50 Hz 采样、转动连续滚动和 60 秒高频唤醒。

如果用户要求直接刷实验包，应先解释上述缺口，并继续做恢复路径或离线验证；不要把“用户愿意承担风险”理解成允许跳过唯一设备的可恢复性验证。
