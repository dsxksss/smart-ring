# 下一位 AI 接手清单（2026-08-26）

## 先读这些文件

依次完整阅读：`AGENTS.md`、本文件、`README.md`、`firmware_research/README.md`、`firmware_research/R08_SENSOR_OBSERVABILITY_20260826.md`、`firmware_research/RT08_OFFLINE_ANALYSIS_20260826.md` 和 `firmware_research/RT08_IMU_ONLY_STREAM_DESIGN_20260826.md`。不要重复已经完成的 APK、OTA 和协议逆向。

工作分支为 `cursor/rust-cross-platform-032b`。用户回家后只使用当前这一枚 R08 戒指，不会购买第二枚测试设备。

## 当前可复核事实

- 设备：`R08_9C07`，`31:31:45:37:9C:07`。
- 硬件/固件：`RT08_V3.1 / RT08_3.10.48_260309`。
- 官方镜像已通过 QRing 访客 OTA 流程取得，长度 `146812`，SHA-256 `c205290a7fcbc816b6be8d40f3e74d533551e0e7f2ebed9090a5d3b1c5ab613b`。
- 官方镜像不是从设备整片读回；已确认应用镜像采用 RTL8762E 格式，真正 SHA 字段在头偏移 `0x394`、原厂为全零且 boot integrity 位关闭。原厂 OTA 接收端会把去掉 `0x50` 包装的镜像写入非活动槽 `0x0084E000..0x00872000`，容量 `0x24000`；`0x00872000` 紧接着是原厂应用至少使用两个 4 KiB 页的持久化存储。另锁定 10 个应用 Flash 区域描述符，覆盖 `0x00874000..0x0087A000` 和 `0x0087B000..0x00880000`；`0x0087A000..0x0087B000` 未分类，且未读取 Flash ID，所以不能把最高地址引用直接写成已确认 512 KiB 物理容量。仍未确认低地址系统分区、OTA Bank Header 更新/选择、运行时失败回退和独立恢复入口。
- 离线手势强度字段补丁已生成，但 `flashAuthorized=false`，绝对不能把它当成可刷版本。
- `A1 04 04` 实测 20.125 秒收到 21 个 `A1 03` 样本，有效频率约 `0.994 Hz`；X 范围 996、Y 范围 196、Z 范围 908。开始和 `A1 02` 停止均成功，没有发送 DFU。
- `A1 01..05` 同步约 1 Hz，说明限制更可能来自上层打包定时器，尚不能断定 LIS3DH 本身只工作在 1 Hz。
- 已重新检查两个 `gsensor_*_timer_id`：`0x00835A63` 与字符串 `gsensor_read_timer_id` 一起传给 `0x00011634`，timer 对象为 `0x0020BF80`；`0x00835A0B` 与 `gsensor_shake_flag_timer_id` 一起传入，timer 对象为 `0x0020BF84`。这两个奇数值落在既有计算函数中段，不能当作可执行回调入口；更准确的定性是 ROM timer 的 ID/上下文参数。Realtek 官方 OS timer FAQ 已确认参数单位是毫秒、最小精度 10 ms，所以 `2000/800/3000` 分别约为 2/0.8/3 秒。
- 已定位 `A1` 主链：`0x0082D0E4 (cmp 0xA1) -> 0x0082D190 -> 0x008280B4`。`A1 04` 分支到 `0x008281D0`，随后在 `0x00828240` 以 `r2=0x7D<<3=1000`、`r1=0x00829DAB` 调用定时包装函数 `0x00829F18`；该包装函数内含 `qc_timer_restart` 字符串。官方 API 语义和实测共同确认这是约 1000 ms 的周期。
- `0x00827DAA` 已确认依次构造并发送 `A1 01..05` 五个 16 字节包，校验函数为 `0x0082AC00`，发送函数为 `0x0082E974`。
- `0x00827DAA` 的唯一直接 `bl` 调用位于 `0x008282EA`，即 A1 处理器公共尾部；`0x00829DAB` 作为 timer 的附加参数传入，但按 Thumb 地址进入会落到低层例程中段，既无独立序言也不直接调用打包函数。当前仍缺 timer 到 A1 处理器/打包器的间接队列分发链，不能把 `1000` 立即数直接当作安全的 BLE 帧率旋钮。
- LIS3DH 的活动采样窗口配置位于 `0x00832D06 -> 0x00832D22`：写 `CTRL_REG1 0x20 = 0x37`（25 Hz、XYZ 开启），并将 `FIFO_CTRL_REG 0x2E` 先写 `0x00`、再写 `0x80` 进入 stream 模式。这才是当前已定位的 FIFO 连续采集配置，但状态机并非让它永久运行。
- `0x008335FC -> 0x0083368C` 的 `CTRL_REG1 0x20 = 0x47`（50 Hz）属于待机动作/唤醒检测配置：同一分支还写入高通滤波、INT1 路由、三轴正向阈值 `0x1F` 和持续时间 `0`，且关闭 FIFO。不能再把 50 Hz 写成全局连续采样率；约 1 Hz 仍不是 LIS3DH 硬件 ODR 上限。
- `0x00833A7C` 的加速度判定路径累计超过阈值后调用 `0x0082D408(2)`；该函数构造并通知的 16 字节包正是 `02 02 00 ... 04`。随后它以 `3000`（3000 ms）重启 `0x0020BF84`（`gsensor_shake_flag_timer_id`）并设置门控标志，因此已确认 `02 02` 至少存在一条 IMU/敲击生成路径，`3000` 对应其约 3 秒门控/冷却期而非 FIFO 采样周期。
- `0x0020BF80` read timer 创建时 repeat=`1`，活动阶段以 `800` 重启；INT1/活动路径 `0x00833C88` 设置状态 `+5`，`0x00833822` 配置 25 Hz FIFO，后续普通周期 `0x0083386A` 调用 `0x00832F9C` 排空 FIFO，状态 `+4` 在 `0x008337FC` 停止。`0x0020BF84` shake timer 创建时 repeat=`0`，与一次性门控用途一致。
- 已新增只读锚点校验 `verify_rt08_imu_stream_anchors.py`，会先验证官方 SHA-256，再验证关键原始指令片段；`analyze_rt08_ota_path.py` 锁定 10 个 OTA 接收端锚点和 3 个相邻存储锚点；`analyze_rt08_boot_activation.py` 锁定 7 个激活链锚点。已证明下载 payload 只有 `image_id=0x2793` 的应用镜像、没有独立 OTA Bank Header，包内 `not_ready/not_obsolete` 均为 `1`；OTA End 以 `(0x2793, 0)` 解析候选地址，验证成功后才提交。应用头 `git_ver` 未直接作为激活参数；ROM API 名称、安装后状态位转换、OTA Bank Header 更新/选择、运行时崩溃回滚和掉电恢复仍明确报告为未证明。Rust 已实现显式 `imu-stream` 命令、`A1 09 01/00` 启停、`A2 10` 解码、姿态滚轮和 fail-closed 清理；默认只监听，必须精确匹配设备身份、显式确认未验证候选并传入 `--inject` 才注入。
- `0x00832F9C` 是按芯片型号分支的批量读取函数；LIS3DH 路径先读 FIFO 状态寄存器 `0x2F`，把有效样本数限制为最多 32 个，再在内部循环 `0x00833120` 从 `0x28` 每次读取 6 字节 XYZ，并写入 `0x0020BF70` 附近的环形缓冲。`0x0083394E` 从该缓冲提取三轴值供 `A1 03` 打包，因此硬件 FIFO、RAM 缓冲消费和 BLE 通知是三个不同速率层。
- 环形缓冲生产者的 16 位字节计数位于 `0x0020BFA8`，每个新 XYZ 前进 6，容量阈值 `0x1EC` 字节（82 样本）。候选每 100 ms 比较一次；不前进就发送一次 `STALE` 并立即关闭 timer/FIFO/IMU，不会重复发送旧姿态。
- 离线 IMU patch 对象占用 `0x00849B08..0x00849C2C`，大小 292，SHA-256 `0aeb8f7fd8ed84e642b38dadfa578d0185fd3aee96a55554ce2798c9a0faec0a`；候选整包 SHA-256 `d55458692d51ff4d21d385e61dfba34c8296934944fe3ad493f0dc07744ec1ac`。构建器锁定原厂/补丁哈希、hook、全零空洞、绝对地址表和外层 sum32，并始终输出 `flash_allowed=false`。
- `0x0083394E` 在 `0x0020BFA1` 非零时会先调用 `0x00832F9C` 排空 LIS3DH FIFO，再从 RAM 环形缓冲取样；补丁设置该标志，因此 100 ms callback 不是重复读取静态 RAM。原厂 callback `0x0083417C` 也会在自身回调中调用 `0x00829F44` 停止同一 timer，降低了补丁自停模式的静态风险，但真实 RTOS 行为仍需非唯一硬件验证。
- 已新增 Unicorn ARMv6-M Thumb 指令级仿真，直接执行哈希锁定的 292 字节补丁并通过 9 个场景：非自定义命令重放、timer 启动失败、启动顺序、新鲜通知、断连后在 FIFO/通知前立即停流、stale 单次通知后急停、120 tick 硬停止、幂等显式停止、停止后迟到 callback。完整回归为 39 个 Python 测试、59 个 Rust 库测试、1 个 Rust 主程序测试，Release 构建成功。
- MP Tool 的 `Backup files` 仅复制工程中已配置的下载文件和 flash map，不会从芯片导出整片 Flash；RTL8762E 的 RD readback 又是加密的且不支持 `Read All`。在非唯一硬件证明原样回写前，任何这类文件都只能标记为“可重复读回证据”，不能叫恢复备份。
- 现有证据只支持用重力向量估计俯仰或横滚；没有陀螺仪证据，绕重力轴的纯旋转不可观测，不能承诺完整“拧戒指”控制。
- R08 应用镜像采用 RTL8762E 格式：`ic_type=12`，镜像基址 `0x00826000`，可执行体起点 `0x00826400`；旧 `0x00824xxx` 映射整体错误 `0x2000`，文件偏移不变。
- ATC_RF03_Ring 使用 BlueX RF03/STK8321，目标 R08 是 RTL8762E/LIS3DH。它只能参考研究方法，不能直接刷固件或复用内存布局、驱动。

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

1. 使用已锁定的 `0x826F2A -> ROM 0x8B94/0x8B7A/0x8A5C/0x3ED1A` 调用链，取得授权的 RTL8762E SDK 符号/头文件来命名 ROM API，并确认 `(0x2793, 0)` 如何更新/选择独立 OTA Bank Header；不要把应用头的 `git_ver` 当作已经证明的激活参数，也不要猜测修改未知版本字段。
2. 恢复应用之外的完整 Flash map、Bootloader/OTA Header/系统配置依赖，以及结构有效但运行时崩溃时的回退行为。
3. 取得真实高清 PCB 正反面照片，确认 RTL8762E 封装及 P0_3、P3_0、P3_1、SWD、GND、VBAT 测试点；现有下载的 FCC `.pdf` 是 Access Denied HTML，不能用于焊盘判断。
4. 在不擦除/不写入的前提下验证 MP/UART 或 SWD 独立进入和 Flash 身份读取；随后才讨论两次一致读回、双介质备份及原样回写演练。
5. 按 `firmware_research/RT08_IMU_ONLY_STREAM_DESIGN_20260826.md` 和 `firmware_research/RT08_TIMER_AND_PATCH_EMULATION_20260826.md` 继续做离线候选差异、真实 RTOS 并发和主机故障注入测试；已有指令级仿真不替代硬件。12 秒固件硬超时、8 秒续期、250 ms 主机超时和 stale 急停均不得放宽。
6. 每个二进制补丁必须同时校验原镜像 SHA-256、精确地址、原始指令字节、Thumb 控制流、QRing 外层 sum32 和 RTL8762E 内层完整性策略；输出差异清单，不连接戒指，不调用 DFU。
7. 在唯一戒指刷写前，识别 MCU/SoC、Boot ROM、量产接口或 SWD 测试点，并证明恢复路径不依赖当前应用固件。没有独立恢复能力就继续离线研究，不刷实验镜像。

## 仍未完成，不能写成已完成

- 没有把任何修改固件刷入戒指。
- 没有从戒指整片读回 Flash，也没有双介质恢复备份。
- 已把 MCU 系列缩小并以镜像头确认到 RTL8762E，但没有确认具体封装、整片分区、Secure Boot、签名或反回滚。
- 没有验证 SWD、JTAG 或 Boot ROM 恢复。
- 已实现但没有刷写或真机验证 25 Hz 采样、10 Hz 通知和姿态连续滚动候选；它不是“可靠可刷”版本。
- 没有确认 OTA Bank Header 更新/选择、运行时失败回滚、完整恢复备份或测试板恢复演练。

如果用户要求直接刷实验包，应先解释上述缺口，并继续做恢复路径或离线验证；不要把“用户愿意承担风险”理解成允许跳过唯一设备的可恢复性验证。

具体只读操作顺序、禁止项和每阶段退出条件见 `firmware_research/RECOVERY_READONLY_RUNBOOK.md`。开始任何拆机、焊接或测试点接触前，仍需取得用户对该物理操作的明确授权。
