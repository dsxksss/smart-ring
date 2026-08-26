# RTL8762E BeeMPTool 静态审计（2026-08-27）

## 范围

本次只读取用户提供的 `RTL8762E_SDK_v1.5.0.zip`，未执行 MPTool、DLL、RTL8762E 目标端代理或其他厂商程序，未打开串口/SWD，也未连接戒指。

| 对象 | 大小 | SHA-256 |
| --- | ---: | --- |
| RTL8762E SDK v1.5.0 | 123872042 | `ef1f47b83d60aeb54edb83a34ecc9d92218965ab18ff02256773441d35f7db52` |
| 内嵌 BeeMPTool v1.1.2.1 ZIP | 35509886 | `57eb7cf9ce3ce7120706f7d7144d9a2cce3b4c33a0409a3784912afadbdfaec4` |
| MP Tool v2.3 英文手册 | 4872246 | `c69ec1d66f0a22e42f2e920ba2463de047117adc2acd13b3f9a006b0add6df5f` |
| `MPTool.exe` | 18672640 | `268177560c6f694aafdd07998c55403cf3fb725159776d41ca02222da25d841d` |
| `rtkmp.dll` | 2117632 | `be098cec366fecc5e8d6d2d5b3783acda46a9b17fc0c383890ba9b9198127430` |
| `RtkSwdMp.dll` | 1029120 | `e2cacddcb2f43cf9e39d6bddeeb190e2b5a0c5d326e2527f33d4b4f6fabc6533` |
| RTL8762E 工具代理 `RTL8762E_FW_B.bin` | 17664 | `0bb4649917a58ed3cbb8c24b19f941f7919bb3e6a83c7b9b881f373621ed1eb6` |

官方 SDK、工具、PDF 和厂商二进制不提交公开仓库。仓库只保存哈希、我们自己的校验器和经过转述的结论。

## 已确认的恢复相关能力

- 新版手册确认烧录 UART 为 `P3_0/UART_TX`、`P3_1/UART_RX`；应用正常模式无法打开时，在复位期间拉低 `P0_3` 可进入 MP mode。
- Debug/RD UI 在该工具包中默认开启，`DLL/EnableButton.switch` 为 `ID_RD_UI_SWITCH=1`。旧文档中的“默认关闭并依赖 RegistrySet”不适用于此版本。
- UART Read 要求 MP mode、`P0_3` 低电平和 4 KiB 对齐地址。手册概览写单次最大 32 MiB，详细章节写 16 MiB；该矛盾必须以更保守的 16 MiB 分块并由目标实测解决。
- 手册只说部分 IC 不支持 `Read All`，未说明 RTL8762E 是否属于其中，也未承诺目标读回是明文还是密文。
- 真实 PE 导出表中，`rtkmp.dll` 包含 `OpenBtMPModulePort`、`ConnectBtMPFlash`、`GetBtMPFlashSize`、`ReadBtMPFlashData`、`ReadBtMPEfuseData`；`RtkSwdMp.dll` 包含 `LoadBootstrap`、`ReadMPFlashData`、`ReadMPEfuseData`。写入/擦除导出也同时存在，因此不能把“加载 DLL”本身视为只读安全保证。
- `RTL8762E_FW_B.bin` 是量产工具使用的目标端代理候选，不是 R08 原厂应用、整片 Flash 备份或可刷戒指固件。
- MPTool、两个 DLL 均无 Authenticode 签名。官方来源由外层 SDK ZIP 哈希保证，但运行风险仍需隔离控制。

## CLI 结论

没有在手册、发布记录或工具目录中发现受支持的 RTL8762E 量产 CLI。`MPTool.exe` 含通用 MFC 命令行字符串，DLL 也暴露函数名，但这不等于存在稳定命令行协议。DLL 没有配套公开头文件、参数契约或只读权限边界；直接猜测调用约定可能误触相邻的 Write/Erase/eFuse 路径。

因此当前不制作“猜参数”的 CLI，也不运行厂商 GUI。后续若 PCB 和电气条件满足，应先在隔离环境中记录 MPTool 的完整只读流程，再据真实调用序列实现仅暴露 Detect/Open/Get Flash ID/Read 的本项目包装器；包装器必须从代码层完全不链接或不导出写入、擦除和 eFuse 操作。

## 尚未通过的硬门槛

- R08 PCB 上 P0_3、P3_0、P3_1、GND、供电和可能的 SWD 测试点未识别；
- 目标 RTL8762E 安全等级、Flash JEDEC ID、容量和保护状态未读取；
- 未证明进入 MP mode 不依赖当前应用且退出后原应用保持正常；
- 未取得两次冷启动、逐字节一致的完整 Flash 读回；
- 未证明读回数据覆盖全部恢复依赖，或可在同芯片原样写回；
- 未在等价非唯一硬件完成擦除、失败启动、写回和恢复演练。

所以这批 SDK 把恢复路线从“猜测接口”推进到“已有官方工具、代理和读取 API 证据”，但没有把候选升级为可刷状态。当前仍为 `NON_FLASHABLE`、`flash_authorized=false`。

机器复核入口：

```powershell
py firmware_research\scripts\verify_rtl8762e_sdk_v1_5_0.py `
  C:\path\to\RTL8762E_SDK_v1.5.0.zip
```

校验器直接读取双层 ZIP、核对 10 个顶层成员和 7 个 BeeMPTool 成员，并解析 DLL 的 PE 导出表；它不会提取后执行厂商工具，也不会连接设备。
