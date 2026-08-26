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

2026-08-27 的第二次交叉验证推翻了旧 `0x394` 结论。RTL8762E SDK v1.5.0 的 `T_IMG_HEADER_FORMAT` 按字段顺序为：96 字节固定前缀、16 字节 `git_ver`、260 字节 RSA 公钥，随后是 `sha256[32]`，因此摘要起点正是 `0x174`。SDK 自带 `prepend_header.exe` 对原厂内层镜像运行后打印下面的摘要，且处理前后逐字节完全一致：

```text
3e143d383a69b749ed928345ac04d517d7aefb95ecc0f2f4eafbe9fd9b146f8f
```

原厂可执行 body 的普通 SHA-256 仍为：

```text
7fbb51ea71c3c656134115d8b561692978c416cdb5f1c8fbdd7aea65c0597d21
```

SDK 工具生成的摘要与普通 body SHA-256 不同，说明它覆盖了额外的认证镜像材料。控制位 `integrity_check_en_in_boot` 未置位只表示 boot 流程不执行该项检查，不代表 OTA End 的 `check_image_chksum` 会跳过摘要验证。v4/v5 修改正文但保留原厂摘要，传输和外层 Check 均成功、激活失败；这正好验证了两层校验的区别。v6 使用精确 SDK ZIP（SHA-256 `ef1f47b83d60aeb54edb83a34ecc9d92218965ab18ff02256773441d35f7db52`）中的 `prepend_header.exe` 只更新 `0x174..0x193`，再重算 QRing 外层 sum32。

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

目标系列 RTL8762E SDK User Guide 的启动图先执行 `Check OTA Headers`，两个 Header 都有效时才进入双 bank 流程；下载章节又把 `OTA Header File` 明确定义为独立于 App Image、用于描述 Flash bank 布局的镜像。双 bank 流程先检查版本更高的 OTA bank，认证/解密失败才检查另一 bank。官方相邻系列 RTL8762C OTA 手册进一步给出每个 bank 独立 4 KiB OTA Header 的字段示例。由此已能确定应用镜像头与 OTA Bank Header 不是同一对象，但仍不能从公开手册确定 R08 ROM 如何比较 Header/应用版本，也没有证明“头部结构有效但进入应用后崩溃”会自动回滚。

`scripts/analyze_rt08_boot_activation.py` 已把原厂 OTA End 与应用内激活链固定为 7 组精确字节锚点：OTA End 以参数 `(image_id=0x2793, second_argument=0)` 调用 `0x00826F2A`，后者到达 ROM `0x00008B94`、`0x00008B7A`、`0x00008A5C`，成功分支再经 `0x00826F16` 调用 ROM `0x0003ED1A`。哈希锁定的 RTL8762E SDK v1.5.0 现已把它们分别命名为 `get_temp_ota_bank_addr_by_img_id`、`is_ota_support_bank_switch`、`check_image_chksum`、本地 `dfu_set_image_ready` 和 ROM `dfu_set_ready`；`0x00826F2A` 对应 `dfu_check_checksum(image_id, offset)`。报告同时锁定 control flags `0x0981`、零 SHA 字段和 `git_ver`：原始 `41 10 00 00 9e a3 01 12` 解码为镜像 git version `1.4.1`、commit ID `0x1201A39E`，但它们没有直接作为激活参数传入。分析器仍固定输出安装后状态、OTA Header 选择、运行时回滚、掉电恢复和刷写授权均为 `false`。

进一步的数据流锚点确认：下载包 payload 只有一个 `image_id=0x2793` 的 RTL8762E 应用镜像，没有附带独立 OTA Bank Header；应用包内 `not_ready/not_obsolete` 两位均为 `1`。激活函数把第二参数保存在 `r7`，由 `get_temp_ota_bank_addr_by_img_id` 的返回值形成候选地址，依据 `is_ota_support_bank_switch` 条件性加 offset，再由 `check_image_chksum` 验证；只有成功才调用 `dfu_set_image_ready`/`dfu_set_ready` 清除暂存头的 `not_ready`。暂存头状态转换已证明，但没有设备 Flash 读回仍不知道 bootloader 复制完成后的安装头状态，也不能证明 OTA Header 是否同时变化。

`scripts/emulate_rt08_stock_activation.py` 又直接执行哈希锁定的原厂 `0x00826F2A..0x00826F87` Thumb 指令，并用确定性桩替代官方符号已命名的 ROM 调用。5 个场景证明：暂存地址解析返回零时不会校验或置 ready；校验失败不会提交；校验成功时 `dfu_set_image_ready` 收到的正是已验证地址；第二参数只在禁用 bank switch 时作为地址偏移加入；特殊 `0xFFFE` 路径先把 `flash_get_bank_addr(5)` 的返回值与 `0x01000000` 合并。该仿真与 SDK 源码共同证明应用侧暂存校验/置 ready 语义，但 ROM 桩没有执行真实 Flash 写入，也不证明复位后 bootloader copy、掉电恢复或回滚。

`scripts/emulate_rt08_stock_image_info.py` 进一步直接执行原厂 `0x00826C22..0x00826C99`，函数区域 SHA-256 为 `487c32ef6ce6a80bb30965141ea4fd611ba2ebd3392e35e79262418d0f40ccdf`。7 个场景确认 `0x2790` 从 `+0x194` 取值，应用 ID 和 `0xFFFE` 从 `+0x60` 取值；两个输出指针均为必需参数，resolver 返回零或 image ID 越界会失败。SDK 源码把该函数精确命名为 `dfu_report_target_fw_info`，ROM `0x8AE2` 为 `get_header_addr_by_img_id`，字段分别为 `T_OTA_HEADER_FORMAT.ver_val` 与 `T_IMG_HEADER_FORMAT.git_ver.ver_info.version`。字段和 API 名称已不再未知；未证明的仍是设备当前安装 Header、bank 选择和运行时回滚。

地址布局同时排除了“下载包本身已经是一份可直接从暂存地址 XIP 的 Bank1 应用”：原厂包的 image base candidate 为 `0x00826000`，`exe_base/load_base` 都是 `0x00826400`；接收端却把内容写到 `0x0084E000..0x00872000`，若直接从该处 XIP，对应入口应为 `0x0084E400`。包内没有第二份重定位应用，也没有独立 OTA Header。SDK 的禁用 bank-switch 参考配置将 Bank1/Bank1 App 大小设为零、另设 OTA TMP；服务端在 Active Image Reset 前置 ready 并解锁 Flash 保护，明确交给 bootloader copy。因此分类提升为 `SINGLE_BANK_COPY_IMAGE_PROVEN_AT_SDK_ARCHITECTURE_AND_R08_APPLICATION_PATH`。这仍不是 R08 bootloader 真实复制和掉电行为的实测；安全判断必须假定旧应用被覆盖，不能把暂存区称作可回滚第二 bank。

因此，ROM API、OTA Header 结构和应用侧 copy 激活路径已从阻塞项中移除。当前真正阻塞刷写的是：R08 实际 Flash Map/安装 Header 尚无只读实证；bootloader 复制中断的掉电恢复矩阵未验证；没有不依赖当前应用的已演练恢复路径；也没有第二枚等价设备完成首次刷写与冷启动。应用头 8 字节已知为 git version/commit ID，但候选仍保持原值，且绝不写入唯一设备。

## 不依赖应用固件的恢复入口

Realtek 官方量产工具文档确认 RTL8762x 支持 UART 下载和 Flash 读回：

- `P0_3`：上电/复位 trap；拉低后复位可绕过 Flash 应用进入 MP 模式；
- `P3_1`：MCU RX；
- `P3_0`：MCU TX；
- PCB 量产测试点通常还应预留 VBAT、GND、LOG/P0_3、TX/P3_0、RX/P3_1；
- SWD 候选为 `P1_0/SWDIO`、`P1_1/SWDCLK`，但具体封装和戒指 PCB 焊盘尚未确认。

SDK v1.5.0 内嵌 BeeMPTool v1.1.2.1。其哈希锁定的 v2.3 手册确认 Read 模式要求 4 KiB 对齐起点，但同一手册的概览和详细章节分别写单次最多 32 MiB、16 MiB，存在文档冲突。手册只说部分 IC 不支持 `Read All`，没有点名 RTL8762E；也没有说明 RTL8762E 的目标回读必然为明文或密文。因此必须由目标只读实测确认读取上限、`Read All` 支持和数据语义，不能沿用旧手册推断。

MP Tool 的 `Backup files` 功能只复制当前工程配置的 RD 下载文件和 flash map，不会从连接的芯片导出完整 Flash。它与 RD readback 是不同功能，两者都不能在缺少回写演练时被称为“设备完整备份”。

该版本 MP Tool 的 RD/Debug UI 默认开启，由 `DLL/EnableButton.switch` 中的 `ID_RD_UI_SWITCH=1` 控制；UART 下载使用 `P3_1/P3_0`，正常模式无法打开端口时可在复位期间拉低 `P0_3` 进入 MP mode。`rtkmp.dll` 和 `RtkSwdMp.dll` 的导出表包含 Flash/eFuse 读取函数，工具包也带有 17664 字节的 RTL8762E 目标端代理，但没有公开稳定 CLI/ABI，且 EXE/DLL 均未签名。本轮只做静态检查，没有执行它们。SDK 文档同时说明 `P0_3` 默认也是应用日志 UART，因此 PCB 上该焊盘可能承担复用功能，不能仅凭单一现象命名测试点。

SDK 内安全机制手册进一步给出恢复路径限制：RTL8762E 安全等级 0 允许 SWD，等级 1-3 均关闭 SWD，等级 3 还禁止 eFuse 读取。R08 应用镜像头 `enc=0` 与手册“APP 加密可选”一致，因此当前 APP body 补丁不需要重新生成 APP AES 密文；但这不能推导目标安全等级为 0，也不能证明 UART MP/RD 或 SWD 在量产设备上开放。任何独立恢复方案都必须先只读取得安全等级/端口可用性，而不是先接线后尝试写入。

## 恢复路径验收门槛

在唯一戒指允许刷写修改镜像前，必须全部完成：

- [ ] 取得 PCB 高清正反面照片并确认具体 RTL8762E 封装；
- [ ] 用万用表和芯片资料确认 P0_3、P3_0、P3_1、GND、供电焊盘，禁止盲接 5 V；
- [ ] 在不擦除的情况下稳定进入 MP 模式并读出芯片/Flash 身份；
- [ ] 明确整片 Flash 分区、系统配置、OTA Header、应用、持久化存储后续区域和 Bootloader 范围；
- [x] 已静态确认活动应用 `0x00826000`、非活动槽 `0x0084E000..0x00872000` 和 QRing 接收端写入上限；
- [ ] 确认 OTA Bank Header 更新/选择、not-ready/not-obsolete 更新和运行时失败回滚语义；
- [ ] 连续读回两次，逐字节一致，分别保存到两个独立介质并记录 SHA-256；
- [ ] 确认目标回读的明文/密文语义并证明原样回写，或取得官方可重建的全部分区镜像；
- [ ] 在第二枚同硬件设备或等价 RTL8762E 测试板上演练擦除、失败启动、恢复、回滚；
- [ ] 验证恢复入口不依赖当前应用固件和 BLE；
- [x] 按 SDK 结构与官方工具实测纠正 SHA 字段到 `0x174`，确认原厂摘要并建立修改后重生成流程；
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
