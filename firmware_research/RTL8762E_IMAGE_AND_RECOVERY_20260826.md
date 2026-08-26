# R08 RTL8762E 镜像与恢复路径（2026-08-26）

## 结论

`RT08_3.10.48_260309.bin` 不是 BlueX RF03 应用。它由 QRing 自定义 `0x50` 字节外层包装和 Realtek RTL8762E 格式的 `0x400` 字节应用头组成。此前按 `0x00824000` 推导的所有 Flash 地址整体少了 `0x2000`；文件偏移和原始字节没有变化。

这项纠正已经写入检查器、反汇编辅助工具和 IMU 锚点验证器。旧的手势强度实验文件没有经过当前 IMU 补丁的逐指令校验、故障停止和恢复验收，因此保持 `flashAuthorized=false`，不得刷写。

## 可重复验证的镜像证据

| 字段 | 原厂镜像值 | 解释 |
| --- | --- | --- |
| 外层 magic | `e5 c3 bd 81` | QRing OTA 包装 |
| 外层头长度 | `0x50` | 两个长度字段与 payload 相等，sum32 有效 |
| Realtek `ic_type` | `12` | Realtek 文档中对应 RTL8762E |
| secure version | `0` | 仅是头字段，不足以证明没有反回滚 |
| control flags | `0x0981` | XIP、not-ready、not-obsolete 置位 |
| image id | `0x2793` | 应用镜像标识 |
| Realtek body 长度 | `145708` | 恰好等于外层 payload 减 `0x400` |
| UUID | `f94c6b7e11c5eb118282f74a0c0cef5b` | 原厂应用头值 |
| image base 候选 | `0x00826000` | 原厂头偏移 `0x28` 的实际值 |
| executable base | `0x00826400` | image base + `0x400` |
| 首条 veneer | `00 48 00 47 65 66 82 00` | `0x00826400` 跳到 Thumb 地址 `0x00826665` |

地址换算公式：

```text
file_offset = 0x50 + (address - 0x00826000)
address     = 0x00826000 + (file_offset - 0x50)
```

例如手势强度补丁位置 `0x0082CA54` 映射到文件偏移 `0x6AA4`。单元测试固定验证该映射，防止再次发生基址误判。

## 内层完整性字段已纠正

早期分析把应用头偏移 `0x174` 的 32 字节误认成 SHA-256。Realtek 官方 `T_IMG_HEADER_FORMAT` 明确该 1024 字节头以 `sha256[32]`、`rsvd2[76]` 结尾，因此真正的 SHA 字段偏移是 `0x400 - 76 - 32 = 0x394`；`0x174` 实际位于 RSA 公钥区域内。

原厂镜像的 `0x394..0x3B3` 全为零，而可执行 body 的普通 SHA-256 为：

```text
7fbb51ea71c3c656134115d8b561692978c416cdb5f1c8fbdd7aea65c0597d21
```

控制位 `integrity_check_en_in_boot` 未置位，`crc16` 也为零，这与禁用应用 body 启动完整性检查的原厂格式一致。当前候选只修改 body，保留整个 1024 字节头（包括零 SHA 字段）并重算 QRing 外层 sum32。这样已经消除了“未知 SHA 生成规则”这一旧阻塞项，但仍不能替代 OTA 写入范围、Bootloader 回退行为和独立硬件恢复验证。

## 原厂 OTA 写入路径

对精确原厂哈希进行只读静态复核后，QRing 接收端已经锁定以下边界：

- 活动应用基址 `0x00826000`；非活动应用槽 `0x0084E000..0x00872000`，容量正好 `0x24000`；
- QRing 外层文件最大 `0x24050`，首个数据块会复制并跳过 `0x50` 字节包装，因此写入槽内的只是 Realtek 镜像；
- 原厂镜像实际占用 `146732` 个槽内字节，剩余 `724` 字节；
- 非活动槽结束地址 `0x00872000` 同时是应用持久化存储的起点。原厂代码以 `base + index * 0x1000` 读写并显式遍历两个页，因此已观察到至少 `0x00872000..0x00874000`；这证明 OTA 槽不能越界增长，但尚不能把后续全部区域命名为完整分区；
- 擦除粒度为 `0x1000`，目标 image id 为 `0x2793`；
- Init 保存请求中的长度、CRC16 和 checksum；可见的 Check 处理器只再次核对已接收长度，而不是重新计算整包 CRC。每个 BLE 外层帧仍单独检查 CRC16；
- End 调用原厂 image activation 例程后由 timer 复位。以上只是协议事实，不是对刷写的授权。

原厂应用还内嵌了 10 个可按精确字节复核的 Flash 区域描述符。去除已单独确认的 `0x00872000..0x00874000` 两页持久化存储后，可观察到：

| 基址 | 长度 | 末端 |
| --- | ---: | --- |
| `0x00874000` | `0x2000` | `0x00876000` |
| `0x00876000` | `0x0800` | `0x00876800` |
| `0x00876800` | `0x0800` | `0x00877000` |
| `0x00877000` | `0x1000` | `0x00878000` |
| `0x00878000` | `0x2000` | `0x0087A000` |
| `0x0087B000` | `0x1000` | `0x0087C000` |
| `0x0087C000` | `0x1000` | `0x0087D000` |
| `0x0087D000` | `0x1000` | `0x0087E000` |
| `0x0087E000` | `0x1000` | `0x0087F000` |
| `0x0087F000` | `0x1000` | `0x00880000` |

这些描述符证明应用会管理或引用 OTA 槽之后的大部分地址空间，并把观察到的最高末端推进到 `0x00880000`；`0x0087A000..0x0087B000` 仍未分类。RTL8762E 官方产品线存在 512 KiB 和 1 MiB Flash 变体，因此在 MP/UART 只读取得 Flash ID 前，不能仅凭最高引用地址宣布物理容量就是 512 KiB，也不能给这些区域擅自命名。`analyze_rt08_ota_path.py` 会验证全部 23 个 OTA、相邻存储与描述符锚点并固定报告 `physical_flash_capacity_proven=false`。

Realtek 官方 SDK 说明双 bank 启动时会先检查版本较高的应用；若认证/解密失败，再检查另一 bank。官方相邻系列 RTL8762C OTA 手册进一步说明，每个 bank 有独立 4 KiB OTA Header，其中保存 bank 版本、各镜像地址和大小；只有 OTA Header 版本高于当前 bank 才被视为可切换的新 bank。该资料不是目标 RTL8762E ROM 的精确实现证明，却说明应用镜像头的 `git_ver` 与 bank 选择版本不能混为一谈，也没有证明“头部结构有效但进入应用后崩溃”会自动回滚。

`scripts/analyze_rt08_boot_activation.py` 已把原厂 OTA End 与应用内激活链固定为 6 组精确字节锚点：OTA End 以参数 `(image_id=0x2793, second_argument=0)` 调用 `0x00826F2A`，后者可到达 ROM `0x00008B94`、`0x00008B7A`、`0x00008A5C`，成功分支再经 `0x00826F16` 调用 ROM `0x0003ED1A`。报告同时锁定 control flags `0x0981`、零 SHA 字段及头偏移 `0x60` 的原始 8 字节版本结构 `41 10 00 00 9e a3 01 12`；这些 `git_ver` 字节并未直接作为激活参数传入。尚无授权 SDK 符号能可靠命名这些 ROM API，也未证明 ROM 如何更新或选择独立 OTA Bank Header；分析器因此固定输出 OTA Bank Header 更新、bank 选择、运行时回滚、掉电恢复和刷写授权均为 `false`。

因此，当前激活阻塞项不再简化表述为“应用 `git_ver` 同版本所以不会切换”，而是：R08 的 ROM 激活 API 参数和 OTA Bank Header 更新/选择语义尚未证明。取得 RTL8762E SDK 中对应 ROM 符号、OTA Header 定义和可复现实验前，不修改应用头那 8 个未知版本字节，也不把候选写入唯一设备。

## 不依赖应用固件的恢复入口

Realtek 官方量产工具文档确认 RTL8762x 支持 UART 下载和 Flash 读回：

- `P0_3`：上电/复位 trap；拉低后复位可绕过 Flash 应用进入 MP 模式；
- `P3_1`：MCU RX；
- `P3_0`：MCU TX；
- PCB 量产测试点通常还应预留 VBAT、GND、LOG/P0_3、TX/P3_0、RX/P3_1；
- SWD 候选为 `P1_0/SWDIO`、`P1_1/SWDCLK`，但具体封装和戒指 PCB 焊盘尚未确认。

MP Tool 的 Read 模式支持 4 KiB 对齐起点和最多 32 MiB 读取，但文档明确指出 RTL8762E 读回数据是加密的，且不支持 Read All。加密物理读回不能自动视为可恢复备份；还需要证明同芯片可原样回写、覆盖范围完整，并且不会漏掉 OTP/eFuse/系统配置依赖。

MP Tool 的 `Backup files` 功能只复制当前工程配置的 RD 下载文件和 flash map，不会从连接的芯片导出完整 Flash。它与 RD readback 是不同功能，两者都不能在缺少回写演练时被称为“设备完整备份”。

MP Tool 文档还确认 RD mode 默认关闭，需 Realtek 套件内的 RegistrySet Tool 开启；UART 下载使用 `P3_1/P3_0`，正常模式无法打开端口时可在复位期间拉低 `P0_3` 进入 MP mode。SDK 文档同时说明 `P0_3` 默认也是应用日志 UART，因此 PCB 上该焊盘可能承担复用功能，不能仅凭单一现象命名测试点。

## 恢复路径验收门槛

在唯一戒指允许刷写修改镜像前，必须全部完成：

- [ ] 取得 PCB 高清正反面照片并确认具体 RTL8762E 封装；
- [ ] 用万用表和芯片资料确认 P0_3、P3_0、P3_1、GND、供电焊盘，禁止盲接 5 V；
- [ ] 在不擦除的情况下稳定进入 MP 模式并读出芯片/Flash 身份；
- [ ] 明确整片 Flash 分区、系统配置、OTA Header、应用、持久化存储后续区域和 Bootloader 范围；
- [x] 已静态确认活动应用 `0x00826000`、非活动槽 `0x0084E000..0x00872000` 和 QRing 接收端写入上限；
- [ ] 确认 OTA Bank Header 更新/选择、not-ready/not-obsolete 更新和运行时失败回滚语义；
- [ ] 连续读回两次，逐字节一致，分别保存到两个独立介质并记录 SHA-256；
- [ ] 证明加密读回数据的原样回写语义，或取得官方可重建的全部分区镜像；
- [ ] 在第二枚同硬件设备或等价 RTL8762E 测试板上演练擦除、失败启动、恢复、回滚；
- [ ] 验证恢复入口不依赖当前应用固件和 BLE；
- [x] 按官方结构纠正 SHA 字段到 `0x394`，确认原厂为全零且 boot integrity 位关闭；
- [ ] 候选镜像通过静态差异、Thumb 控制流、超时停止、断连停止和功耗测试。

用户目前只有唯一一枚戒指。以上未满足前，仅允许只读分析、离线候选和主机端仿真；禁止发送 DFU Start/Init/Data/Check/End，也禁止通过 MP/SWD 擦除或写 Flash。

## 官方资料

- Realtek RTL87x2G SDK User Guide（包含应用镜像头格式）：<https://www.realmcu.com/img/ipb/en_638358166065142992.pdf>
- Realtek RTL8762C OTA User Manual（仅作相邻系列 OTA Header 结构证据）：<https://www.realmcu.com/img/ipd/en_638290111802009694.pdf>
- Realtek MP Tool User Guide（UART 进入、下载与读回）：<https://www.realmcu.com/img/ipd/en_638115612700983899.pdf>
- Realtek Hardware Instruction：<https://www.realmcu.com/img/ipg/en_638357619405474080.pdf>
- FCC R08 Internal Photos：<https://fccid.io/2AOM3-R08/Internal-Photos/Internal-photos-7833424.pdf>

FCC 索引确认内部照片附件存在（3 页、报告号 `CTL2408273012-WF`、附件 id `7833424`），但本机此前保存的同名 `.pdf` 实际是 Cloudflare/Access Denied HTML，不是照片，不能作为焊盘证据。取得真正高清附件或由用户明确授权拆机并拍摄 PCB 宏观照片前，不猜测测试点。

本文件记录的是恢复设计和硬门槛，不是对刷写的授权。

官方 PDF 的本地复核哈希、页码和保守解释见 `OFFICIAL_REALTEK_EVIDENCE_20260826.md`。

唯一设备的分阶段验收和逐项恢复能力矩阵见 `RECOVERY_READONLY_RUNBOOK.md`。
