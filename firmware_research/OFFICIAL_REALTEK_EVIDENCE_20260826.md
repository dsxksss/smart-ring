# RTL8762E 官方资料复核记录（2026-08-26）

## 文件身份

两份文件均从 RealMCU 官方公开 URL 下载，只用于本地只读研究，不提交 PDF 本体到公开仓库。

| 文件 | 官方 URL | 大小 | SHA-256 |
| --- | --- | ---: | --- |
| RTL8762E SDK User Guide | `https://www.realmcu.com/img/ipb/en_638358166065142992.pdf` | 1,817,869 | `585458310be551a4831a34fb3ecad93fd116e90d0a3002fb39dc06f36e3d5640` |
| RTL8762x MP Tool User Guide | `https://www.realmcu.com/img/ipd/en_638115612700983899.pdf` | 4,188,756 | `c2b7220bfdcc1c0afe47c5c593f138bf41409bfbccaf730b1068645307153da5` |

PDF 已用 Poppler 渲染并对下列相关页做视觉复核；不是只依赖文本抽取。

2026-08-27 又取得 RealMCU 提供的 `RTL8762E_SDK_v1.5.0.zip`。本地文件大小为 `123872042` 字节，SHA-256 为 `ef1f47b83d60aeb54edb83a34ecc9d92218965ab18ff02256773441d35f7db52`。SDK 本体和厂商源码不提交公开仓库；`scripts/verify_rtl8762e_sdk_v1_5_0.py` 会直接读取 ZIP、核对 7 个关键成员的大小和 SHA-256，不解压、不执行厂商程序，也不连接设备。

## SDK v1.5.0 源码与 ROM 符号能证明的事项

- `bin/gcc/rom_symbol_gcc.axf` 是文本形式的官方 ROM 符号表。去掉 Thumb 位后，R08 已观察地址可精确命名为：`0x80B8 flash_get_bank_addr`、`0x8A5C check_image_chksum`、`0x8AE2 get_header_addr_by_img_id`、`0x8B7A is_ota_support_bank_switch`、`0x8B94 get_temp_ota_bank_addr_by_img_id`、`0x3ED1A dfu_set_ready`。
- R08 的 `0x00826C22` 与 SDK 的 `dfu_report_target_fw_info` 参数、分支和字段访问一致：`image_id=0x2790` 读取 `T_OTA_HEADER_FORMAT.ver_val`（偏移 `0x194`），应用 image ID 读取 `T_IMG_HEADER_FORMAT.git_ver.ver_info.version`（偏移 `0x60`）。此前未命名的两个字段和 ROM resolver 现已由目标 SDK 精确命名。
- `T_VERSION_FORMAT` 也已给出完整位域：R08 应用头 `41 10 00 00 9e a3 01 12` 可解码为镜像 git version `1.4.1`、commit ID `0x1201A39E`。它不是 OTA End 的直接参数，也不能据此推断商品展示版本 `RT08_3.10.48_260309` 的比较规则。
- R08 的 `0x00826F2A` 与 SDK `dfu_check_checksum(image_id, offset)` 的签名和控制流一致：取得暂存地址，非 bank-switch 时加 offset，调用 `check_image_chksum`，成功后经 `0x00826F16`/`dfu_set_image_ready` 调用 ROM `dfu_set_ready`。因此“校验暂存镜像并清除其 `not_ready` 位”的应用侧激活路径已证明。
- SDK 的 `dfu_service_handle_active_image` 明确在禁用 bank switch 时遍历暂存镜像并置 ready；`DFU_OPCODE_ACTIVE_IMAGE_RESET` 随后解锁 Flash block protection，注释说明这是为了复位后的 bootloader OTA copy。
- SDK 自带的禁用 bank-switch 参考布局把 Bank1 大小和 Bank1 App 大小设为零，并单独配置 OTA TMP。这证明 RTL8762E 官方单 bank 设计确实是“暂存、置 ready、复位后复制”，而不是从临时地址直接 XIP。

以上是对目标 SoC 软件架构和 R08 应用内调用链的强证据，但 SDK 的参考 Flash Map 不是 R08 的实际 Flash Map；bootloader 在 R08 上的真实复制范围、掉电断点、失败恢复和安装后头部仍须通过等价非唯一硬件或独立只读恢复链验证。

## SDK User Guide 能证明的事项

- PDF 第 23-24 页（印刷页 15-16）描述 RTL8762E 双 bank 启动：先选择版本更高的 OTA bank，镜像检查/解密失败才检查另一 bank。
- PDF 第 23 页的启动图先执行 `Check OTA Headers`，在两个 OTA Header 都有效时才进入 `Dual Bank Process`；第 41 页又明确 `OTA Header File` 是独立镜像，用来定义 Flash bank 布局。因此 R08 应用头和 OTA Bank Header 是不同对象。
- Realtek 官方 OTA 方案说明：真正 bank 切换需要为 Bank1 地址另行编译应用，并连同 OTA Header 及 bank 内所需镜像一起打包；不切 bank 的方案则把 OTA TMP 中通过验证的镜像搬回 Bank0。R08 包只含链接到 `0x00826000/0x00826400` 的应用，却暂存到 `0x0084E000`；单独的静态布局分类仍为 `SINGLE_BANK_COPY_IMAGE_CONSISTENT`，结合 SDK 源码/符号后的整体分类为 `SINGLE_BANK_COPY_IMAGE_PROVEN_AT_SDK_ARCHITECTURE_AND_R08_APPLICATION_PATH`，但不能假设旧应用仍保留可回滚。
- PDF 的 `T_IMG_HEADER_FORMAT` 精确定义 `git_ver` 位于应用镜像头偏移 `0x60`；SDK 源码进一步把 OTA Header ID `0x2790` 的 `+0x194` 字段命名为 `T_OTA_HEADER_FORMAT.ver_val`。
- 该流程图只覆盖启动时镜像检查和解密。它没有说明两个 bank 版本完全相同时选哪一个，也没有说明结构有效但应用运行后 HardFault 是否回滚。
- PDF 第 42-43 页（印刷页 34-35）给出 1024 字节应用头结构：`not_ready`、`not_obsolete`、`integrity_check_en_in_boot` 位，以及 `T_VERSION_FORMAT git_ver`、RSA 公钥、SHA-256 和保留区的顺序。
- 手册没有展开 `T_VERSION_FORMAT`，但 SDK 头文件已展开其位域；R08 的这 8 字节是镜像 git version 与 commit ID。它仍不得当作商品固件字符串或在没有更新策略证据时擅自修改。
- SDK 文档说明 `P0_3` 默认用作应用日志 UART。这与 MP mode trap 使用同一引脚并不矛盾，但意味着 PCB 测试点必须结合复位时序和走线确认。

## OTA Header 补充证据

目标系列 SDK 的 `T_OTA_HEADER_FORMAT` 已精确定义 OTA Header，包括 `ver_val` 及各镜像地址/大小；SDK User Guide 同时证明 OTA Header 与应用镜像头是不同对象。RealMCU 官方 RTL8762C OTA User Manual：`https://www.realmcu.com/img/ipd/en_638290111802009694.pdf` 仍只作相邻系列补充。SDK 结构不能替代对 R08 当前安装 OTA Header 的真机只读验证。

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
- R08 当前安装 OTA Header、实际 Flash Map、bootloader 复制范围和掉电原子性；
- 应用已经启动后发生 HardFault/死循环时是否存在自动回滚。

因此这些官方资料加强了恢复设计，但不改变候选的 `NON_FLASHABLE` / `flash_allowed=false` 状态。

补丁本身的 timer 契约、FIFO 消费链和 Cortex-M0 指令级仿真见 `RT08_TIMER_AND_PATCH_EMULATION_20260826.md`。这些离线结果同样不能替代真实 RTOS、功耗和恢复测试。
