# R08 IMU-only patch object（离线草案）

这里生成的是定位到 `0x00849B08` 的 Cortex-M0+ 补丁对象，**不是可刷固件**。构建脚本只编译、链接到本地 `build/`，不会修改原厂镜像或连接戒指。原始全零区从 `0x00849B06` 开始；代码刻意前移 2 字节以满足 Thumb literal 的 4 字节对齐。

当前草案通过原厂 `A1` 处理器的无效子命令路径预留 `A1 09 01/00` 启停命令，使用独立 100 ms timer callback、原厂 25 Hz LIS3DH FIFO、原厂环形缓冲消费者和 NUS 通知函数，输出既定的 `A2 10` 16 字节 IMU 包。它有 12 秒固件硬停止；主机每 8 秒显式续期，断连后不会无限运行。每次回调还比较原厂环形缓冲的 16 位生产者字节索引；100 ms 内索引未前进即发送一次 `STALE` 包并关停，不会把旧样本重复伪装成连续数据。

尚未满足以下条件，因此禁止生成或刷写 OTA：

- 验证补丁范围不超过保守候选空洞 `0x00849B06..0x00849C30`；当前 280 字节对象占用 `0x00849B08..0x00849C20`，尾部保留 16 个零字节；
- 逐条反汇编已核对 hook、绝对地址、栈平衡、失败时不启用 IMU、生产者索引无变化时 `STALE` 急停，以及 12 秒异常停止；构建器锁定经审计补丁 SHA-256 `a157de7d40619ac9efa259e04473f7e7929ed2c82f9a9a77f25b2fc217a55928`；
- 原厂 timer 包装和 Realtek 官方六参数 `os_timer_create` 原型相互吻合；仍需在非唯一硬件验证 callback 内删除/重建；
- 原厂状态机本身顺序调用 `0x00832CBC` / `0x008335FC` 停止 FIFO 与待机，静态路径吻合；仍需真机功耗验证；
- 恢复精确 Flash map、OTA 写入/回退行为和独立 UART/SWD 回滚路径。内层 SHA 字段已纠正到 `0x394`，原厂值全零且 boot integrity 位关闭。

构建：

```powershell
powershell -ExecutionPolicy Bypass -File firmware_research\patches\r08_imu_stream\build.ps1
```
