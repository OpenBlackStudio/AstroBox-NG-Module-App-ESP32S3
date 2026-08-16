# AstroBox-NG-Module-App-ESP32S3

针对嵌入式硬件 ESP32-S3 的 AstroBox-NG 客户端

## 特性

### 多设备同时连接
- 同时连接多台小米穿戴设备（最多 5 台，通过 `CONFIG_BT_NIMBLE_MAX_CONNECTIONS` 配置）
- 支持设备型号：
  - **小米手环系列**: Mi Band 10 Pro / 10 / 9 Pro / 9
  - **Redmi Watch 系列**: Redmi Watch 6 / 5 / 5 eSIM
  - **小米 Watch 系列**: Xiaomi Watch S5 / S4 / S3
- 基于 FE95 服务 UUID 的通用匹配 + 设备名称关键字模糊匹配
- 按设备地址（MAC）稳定追踪，避免索引偏移导致的错误断开
- 单台设备断开仅重连该设备，其他设备保持连接

### 网络与连接
- Wi-Fi 自动重连看门狗（10 秒间隔检测）
- Wi-Fi 凭据 NVS 持久化存储（SSID / 密码跨重启保存）
- Wi-Fi 初始化重试（最多 5 次，线性退避）
- BLE (MiWear) 自动重连，支持指数退避（5s → 10s → 20s → ... → 最长 120s）

### 显示与交互
- GC9A01 240×240 圆形屏幕显示
- CST816S 触摸支持
- 电量、充电状态、网络速度实时监控
- 多设备连接计数 UI

### 其他
- 伪 ANCS (Apple Notification Center Service) BLE 服务
- OTA 更新能力 stub（预留接口）
- ESP32-S3 PSRAM 优化
- Slint UI 渲染框架

## 编译

以下教程基于 macOS Tahoe 26 (aarch64)

先安装：cmake、ninja、dfu-util、ldproxy（通过 cargo install）

必须使用 esp-idf v5.3.3，执行编译时会自动下载，但推荐你先自己装好 idf v5.3.3，然后使用 zed 编辑器并 install cli，在终端中先执行 idf 的 export.sh，接着直接 `zed -n <folder path>` 打开项目以节省时间

> 注意：该模块依赖独立的交叉编译工具链，已经从 `src-tauri` 顶层 Cargo workspace 中剥离。请直接进入该目录后再运行 Cargo 命令。为了让 rust-analyzer 正常工作，你通常也需要在编辑器中单独打开该模块的文件夹。

## 编译命令

```bash
# Debug
cargo build

# Release
cargo build --release

# 烧录
espflash flash -B 1500000 ./target/xtensa-esp32s3-espidf/release/app_esp32s3

# 生成固件镜像 (含分区表)
bash save_image_rel.sh
```

## 硬件要求

| 项目 | 规格 |
|------|------|
| MCU | ESP32-S3 |
| Flash | 16MB |
| PSRAM | OCT 80MHz |
| 显示屏 | GC9A01 240×240 圆形 |
| 触摸 | CST816S |
| 蓝牙 | NimBLE（最多 5 路并发连接） |

## 配置

WiFi 凭据可通过编译时环境变量或运行时 NVS 存储配置：

```bash
# 编译时设置（在 .env 或环境变量中）
export WIFI_SSID="your_ssid"
export WIFI_PASSWORD="your_password"
cargo build --release
```

首次启动时如果未配置凭据，系统会使用默认值；成功连接后，凭据会自动保存到 NVS，下次启动直接读取。

此库使用 AGPL 3.0 授权

This library is licensed under AGPL 3.0

## 额外条款 / Additional Terms
根据 AGPL 3.0 所述可选附加条款，本项目额外附加署名要求，使用此项目需在遵守 AGPL 3.0 条款后额外为此项目添加署名，署名包括但不限于本项目仓库地址，作者名等。

注：附加条款以中文版为准，其他语言仅供参考！

According to the optional additional terms stated in AGPL 3.0, this project includes an additional attribution requirement. When using this project, after complying with the terms of AGPL 3.0, you must also add attribution for this project, which includes but is not limited to the project repository address, the author's name, etc.

Note: The additional terms are based on the Chinese version. Other languages are for reference only!