# R08 跨电脑研究资料包

这是用户明确要求提交到仓库的可迁移研究资料，供换电脑后继续分析。仓库应尽快调整为私有；仅修改可见性不会清除已有 Git 历史。

## 内容

- `firmware/`：精确匹配 `RT08_V3.1 / RT08_3.10.48_260309` 的官方固件、离线实验候选及不含认证信息的元数据。
- `sensor_capture/`：唯一戒指的 R08 专用脱敏日志和三轴 CSV。
- `qring_apk/`：QRing 访客模式 APK 的 64 MiB 分卷。
- `qring_decompiled/`：完整 apktool 反编译目录压缩包的 64 MiB 分卷。
- `tools/`：本次分析实际使用的 apktool、Capstone wheel、csengine 和 Windows Python 3.12 Bleak 运行依赖。
- `reference/`：ATC_RF03_Ring 参考项目快照。它使用 STK8321，而目标 R08 固件指向 LIS3DH，不能直接刷入或照搬传感器驱动。

未提交 ADB 无线配对码、临时端口、账号令牌、手机通知、完整系统日志及附近无关 BLE 设备扫描数据。

## 恢复 APK 和反编译压缩包

在仓库根目录运行：

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\restore_research_artifacts.ps1
```

默认输出到被 Git 忽略的 `tmp/restored-research-artifacts/`。脚本拒绝覆盖已有文件，并校验原始长度和 SHA-256；它不会安装 APK、解压反编译目录或连接/刷写戒指。

## 原始大文件校验

| 恢复文件 | 长度 | SHA-256 |
| --- | ---: | --- |
| `qring-guest-analysis-20260826.apk` | 131757887 | `8b8f60209ba3dde47d803bdfc5852d951efc91377b6eff85ca110cc3ba4d1ddf` |
| `qring-guest-smali-20260826.zip` | 139332010 | `e06be7453ed6027388e392b3dfb2a13219522abc35fc3e53fd0ba9a51725dc53` |

官方固件 SHA-256 为 `c205290a7fcbc816b6be8d40f3e74d533551e0e7f2ebed9090a5d3b1c5ab613b`；实验镜像 SHA-256 为 `2a382a1edd756997d22a2d04a8448cf6f8e14f0deba243de44a2cd52207d20a9`。全部仓库内文件的逐文件校验见 `MANIFEST.sha256`。
