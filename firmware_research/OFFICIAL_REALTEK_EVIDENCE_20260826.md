# RTL8762E 官方资料复核记录（2026-08-26）

## 文件身份

两份文件均从 RealMCU 官方公开 URL 下载，只用于本地只读研究，不提交 PDF 本体到公开仓库。

| 文件 | 官方 URL | 大小 | SHA-256 |
| --- | --- | ---: | --- |
| RTL8762E SDK User Guide | `https://www.realmcu.com/img/ipb/en_638358166065142992.pdf` | 1,817,869 | `585458310be551a4831a34fb3ecad93fd116e90d0a3002fb39dc06f36e3d5640` |
| RTL8762x MP Tool User Guide | `https://www.realmcu.com/img/ipd/en_638115612700983899.pdf` | 4,188,756 | `c2b7220bfdcc1c0afe47c5c593f138bf41409bfbccaf730b1068645307153da5` |

PDF 已用 Poppler 渲染并对下列相关页做视觉复核；不是只依赖文本抽取。

## SDK User Guide 能证明的事项

- PDF 第 23-24 页（印刷页 15-16）描述 RTL8762E 双 bank 启动：先选择版本更高的 OTA bank，镜像检查/解密失败才检查另一 bank。
- 该流程图只覆盖启动时镜像检查和解密。它没有说明两个 bank 版本完全相同时选哪一个，也没有说明结构有效但应用运行后 HardFault 是否回滚。
- PDF 第 42-43 页（印刷页 34-35）给出 1024 字节应用头结构：`not_ready`、`not_obsolete`、`integrity_check_en_in_boot` 位，以及 `T_VERSION_FORMAT git_ver`、RSA 公钥、SHA-256 和保留区的顺序。
- 手册没有展开 `T_VERSION_FORMAT` 字段定义，因此 R08 头偏移 `0x60` 的 `41 10 00 00 9e a3 01 12` 仍不得当作普通版本号递增。
- SDK 文档说明 `P0_3` 默认用作应用日志 UART。这与 MP mode trap 使用同一引脚并不矛盾，但意味着 PCB 测试点必须结合复位时序和走线确认。

## 相邻系列 OTA Header 证据

RealMCU 官方 RTL8762C OTA User Manual：`https://www.realmcu.com/img/ipd/en_638290111802009694.pdf`。该手册说明每个 bank 具有独立 4 KiB OTA Header，包含 bank 版本以及各镜像地址和大小；新 bank 的 OTA Header 版本需高于当前 bank 才有效。它只用于区分“应用镜像头 `git_ver`”与“OTA Bank Header 版本”这两个概念，不能替代 RTL8762E SDK/ROM 的精确语义，也不能证明 R08 会如何更新或选择 Bank Header。

## MP Tool User Guide 能证明的事项

- PDF 第 9 页（印刷页 1）确认 UART 下载需要 `P3_1` 和 `P3_0`，RD mode 默认关闭，需要 Realtek 套件内 RegistrySet Tool 开启。
- PDF 第 14-15 页（印刷页 6-7）列出 RD mode 的读取、Flash ID 和擦除能力；RTL8762E 的 Flash 读回是加密数据，按地址读取要求 4 KiB 对齐，单次最大 32 MiB。
- PDF 第 51 页（印刷页 43）说明正常模式无法打开端口时，复位期间拉低 `P0_3` 可切换到 MP mode。
- PDF 第 53 页（印刷页 45）再次确认 UART 读回要求芯片处于 MP mode；RTL8762E 不支持 `Read All`，只能按起点和长度读取加密数据。
- PDF 第 58 页（印刷页 50）给出 RD mode 的 Flash ID 只读流程：detect、open、get flash ID。
- 文档中的 `Backup files` 只复制当前配置的 RD 下载文件和 flash map，不会从目标芯片读取整片 Flash；它不能充当设备备份。

## 这些证据仍不能证明的事项

- R08 PCB 上哪些焊盘实际连接到 `P0_3/P3_0/P3_1`，以及其电平和供电拓扑；
- R08 外部 Flash 的 JEDEC ID、实际容量、保护状态和完整分区；
- RTL8762E 加密读回数据能否在同一芯片上原样回写，并在应用损坏后恢复；
- R08 的 OTA Bank Header 更新/选择规则、掉电原子性和应用运行时故障回滚；
- ROM `0x8B94/0x8B7A/0x8A5C/0x3ED1A` 的精确符号和参数语义。

因此这些官方资料加强了恢复设计，但不改变候选的 `NON_FLASHABLE` / `flash_allowed=false` 状态。

补丁本身的 timer 契约、FIFO 消费链和 Cortex-M0 指令级仿真见 `RT08_TIMER_AND_PATCH_EMULATION_20260826.md`。这些离线结果同样不能替代真实 RTOS、功耗和恢复测试。
