# RT08 timer 契约与 IMU 补丁指令级仿真（2026-08-26）

本文只记录对原厂 OTA 镜像和本地 `NON_FLASHABLE` 候选的离线验证；没有连接、写入或升级戒指。

## 原厂 timer 包装函数

目标镜像 SHA-256：

`c205290a7fcbc816b6be8d40f3e74d533551e0e7f2ebed9090a5d3b1c5ab613b`

`0x00829F18` 的控制流已经逐条核对：

- timer handle 非空时，将周期放入 `r1`，把 `&handle` 放入 `r0`，调用 ROM `0x00013694`，随后直接 `pop` 返回；
- handle 为空时，以 `&handle / "qc_timer_restart" / 1 / period / repeat / callback` 调用 ROM `0x00013634`，再以 `&handle` 调用 ROM `0x00013670`，随后直接 `pop` 返回；
- 因为最后一条 ROM 调用之后没有改写 `r0`，包装函数的返回值就是 ROM start/restart 的返回值；原厂镜像中的 26 个直接调用者均没有消费这个返回值；
- 现有补丁检查返回值为非零后才打开 IMU，这比原厂调用者更保守。Realtek 相邻系列公开示例一致使用 `os_timer_create`、`os_timer_start` 接口，但目标 RTL8762E SDK 的精确函数声明仍需从登录后的官方 SDK 取得，不能只凭相邻系列把返回类型写成已完全证明。

停止包装 `0x00829F44` 在 handle 非空时依次调用 ROM `0x000136BC` 和 `0x000136E0`。原厂回调 `0x0083417C` 在倒计时结束时，用同一 timer handle 调用该停止包装；对应启动路径在 `0x0083421E` 把该回调注册到同一 handle。因此“软件 timer 在自己的 callback 内停止/删除”不是补丁臆造的新模式，而是目标固件已经使用的控制流。真机 RTOS 行为仍应先在非唯一设备上验证。

## FIFO 生产/消费闭环

补丁调用的原厂消费者 `0x0083394E` 会检查 `0x0020BFA1`：该标志非零时，它先调用 `0x00832F9C` 排空 LIS3DH 硬件 FIFO，再从 `0x0020BFA0` 环形缓冲复制所需样本。补丁启动时把该标志设为 `1`，所以 100 ms callback 不是只读旧 RAM；它会先执行原厂 FIFO 排空路径，然后比较 `0x0020BFA8` 的生产者字节计数。生产者不前进时发送一次 `STALE` 后完整停止。

通知包装器 `0x0082E974` 的 52 字节原厂控制流也已加入精确锚点：它先调用 `0x0082DCFE` 检查连接，未连接时保持 `r0=0` 直接返回；已连接时发送固定 16 字节通知，最后返回另一原厂例程的结果。由于该最终返回值的精确语义没有官方符号支持，补丁不再依赖它判断发送成功，而是在每个 tick 开头直接调用 `0x0082DCFE`；断连时会在读取 FIFO 和通知前立即进入统一停止路径，关闭 timer、FIFO 和 IMU。

## Cortex-M0 指令级仿真

`firmware_research/scripts/emulate_rt08_imu_stream_patch.py` 使用 Unicorn 的 ARMv6-M Thumb 模式执行经 SHA-256 锁定的 292 字节补丁对象。原厂函数调用由可审计的契约 stub 代替，验证真实机器码分支、栈恢复、RAM 写入和通知包，而不是重新实现一份高层伪代码。

本轮通过 9 个场景：

1. 非 `A1 09` 命令精确重放被 hook 覆盖的两个 `STRB`；
2. timer 启动失败时绝不打开 IMU；
3. 启动顺序为先幂等清理、再启动 timer、最后打开 25 Hz IMU；
4. 新鲜样本生成 `A2 10` 帧、正确小端 XYZ 和校验，并推进序号；
5. 断连时在读取 FIFO 或通知前立即停止 timer、FIFO 和传感器；
6. 生产者不前进时只发一个 `STALE`，随后停止 timer、FIFO 和传感器；
7. 第 120 个 100 ms tick 强制完整停止；
8. 显式 stop 清空状态且资源释放完整；
9. 已停止状态下的迟到 callback 无副作用。

复现命令：

```powershell
python -m pip install -r firmware_research\requirements-analysis.txt
$env:PYTHONPATH = 'firmware_research\scripts'
python firmware_research\scripts\emulate_rt08_imu_stream_patch.py `
  firmware_research\patches\r08_imu_stream\build\r08_imu_stream.bin
```

这些结果提高了候选补丁本身的可信度，但不改变其分类：它仍是 `NON_FLASHABLE_OFFLINE_CANDIDATE`。指令仿真无法证明真实 RTOS 调度、BLE 堆栈时序、电源行为、Flash bank 激活或变砖恢复。
