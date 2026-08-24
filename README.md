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
- 断连事件使用 mpsc 队列排队，多设备同时断连不会丢事件
- BLE 扫描结果按 MAC 去重，同一设备不会被记录数十次

### 快应用与表盘管理
- **快应用安装**: 向设备推送 `.pk` 格式快应用包（基于 `MassDataType::ThirdPartyApp`）
- **表盘安装**: 向设备推送 `.bin` 格式表盘文件（基于 `MassDataType::Watchface`）
- **安装进度**: 通过 BLE Mass 协议分块传输，自带 MD5 校验和 CRC32 完整性验证
- **列表查询**: 查询设备已安装的快应用列表和表盘列表
- **快应用管理**: 启动、卸载快应用
- **表盘管理**: 切换当前表盘、卸载表盘
- **电话消息桥接**: 向设备上的快应用发送电话端消息
- 周期性（30秒）自动同步各设备的已安装项目状态

#### API 一览

```rust
// 快应用
install::install_quick_app(addr, package_name, data).await?;
install::install_quick_app_from_file(addr, package_name, file_path).await?;
install::uninstall_quick_app(addr, package_name).await?;
install::launch_quick_app(addr, package_name).await?;
install::list_installed_quick_apps(addr).await?;
install::send_phone_message(addr, package_name, payload).await?;

// 表盘
install::install_watchface(addr, data).await?;
install::install_watchface_from_file(addr, file_path).await?;
install::uninstall_watchface(addr, watchface_id).await?;
install::set_watchface(addr, watchface_id).await?;
install::list_installed_watchfaces(addr).await?;
```

### 设备间数据传输
- **快应用跨设备复制**: 从设备 A 读取快应用包，安装到设备 B
- **表盘跨设备复制**: 从设备 A 读取表盘数据，安装到设备 B
- **应用消息桥接**: 将一个设备上的快应用消息转发到另一个设备的同名快应用
- **互联消息中继**: 实时监听设备的互联消息事件，自动中继到目标设备
  - 拒绝 `src == dst`，自带 FNV-1a payload 去重，不会形成 A↔B 转发死循环
- **数据广播**: 向所有已连接设备发送同一份数据
- **设备资源管理**: 查询设备列表、获取设备名称
- 传输进度回调支持（Mass 文件分块传输）

#### API 一览

```rust
// 文件/数据传输
transfer::send_data_to_device(addr, data_type, data).await?;
transfer::send_data_to_device_with_progress(addr, data_type, data, cb).await?;
transfer::broadcast_data_to_all_devices(data_type, data).await?;

// 内容复制
transfer::transfer_quick_app_between_devices(src, dst, pkg).await?;
transfer::transfer_watchface_between_devices(src, dst, face_id).await?;

// 消息桥接
transfer::forward_app_message(src, dst, pkg, payload).await?;
transfer::relay_interconnect_message(src, dst).await?;  // 返回 JoinHandle

// 设备管理
transfer::list_connected_devices().await;
transfer::get_device_info(addr).await?;
```

### 网络与连接
- Wi-Fi 重连看门狗：阻塞调用移至独立 OS 线程（`wifi-wd`），**不会冻结单线程 Tokio 运行时**，因此重连期间 UI/触摸/BLE 保持响应
- Wi-Fi 凭据 NVS 持久化存储（SSID / 密码跨重启保存）
- Wi-Fi 初始化重试（最多 5 次，线性退避）
- BLE (MiWear) 自动重连，支持指数退避（5s → 10s → 20s → ... → 最长 120s）

### 显示与交互
- ST7789 240×320 方形屏幕显示（GC9A01 圆屏引脚兼容，可回切）
- CST816S 触摸支持（I²C 400kHz，INT 上拉，RST 引脚）
- 电量、**充电状态（实时刷新）**、网络速度实时监控
- 多设备连接计数 UI
- **安装资源面板**：**长按 ⚙ 设置按钮 600 ms** 呼出安装资源面板，两个 Tab 可切换 `本地 (SD) / AstroBox 官方源`，5 行列表 + 上一页/下一页分页，点击行即触发安装，顶部显示实时进度文本。

### MicroSD 日志 & 本地安装（FAT32 / SPI）
- **滚动日志输出到 SD 卡**：同时输出 USB 串口与 `/sdcard/logs/astrobox_YYYYMMDD_NNNN.log`；单文件 ≥ 512 KB 自动切分，总占用 > 4 MB 或 mtime > 7 天自动清理；连续写失败 ≥ 5 次自动禁用 SD 日志（仅 warn 一次，**不 panic，不影响主流程**）。
- **从 MicroSD 安装快应用 / 表盘**：把安装包放入 `/sdcard/astrobox/packages/`，目录不存在会自动创建；支持扩展名：
  - `.rpk` → 快应用（自动尝试从 ZIP 包 `manifest.json` 取 `package_name`，失败用文件名兜底）
  - `.mwz` / `.face` → 表盘
  - `.bin` → 资源二进制（预留入口）
- **空闲空间保护**：写缓存 / 安装前查询 `statvfs`，剩余 **< 32 MB 警告，< 8 MB 拒绝写入**，避免写坏 FAT 表。
- SNTP 时间：Wi-Fi 连接成功后 best-effort 同步一次 `pool.ntp.org`（sdkconfig 已开 `CONFIG_LWIP_SNTP_ENABLED`）；失败时日志文件名退化为 `epoch-秒数`，不阻塞。

### 联网仓库接入 & 网络安装（自动过滤付费）
- **AstroBox 官方源**：直连 `AstralSightStudios/AstroBox-Repo` 的 `index.csv`（主源 GitHub Raw，失败自动回退 CDN jsDelivr），解析 CSV 列 `name, icon, cover, restype, tags, devices, path, paid_type`。
- **关于「米坛社区（BandBBS）」源**：此前曾计划"公开 HTML 抓取 + 关键词过滤"，但 BandBBS 用户协议 / 服务条款**明确禁止未授权的自动化抓取与爬虫访问**，因此相关实现**已从本仓库彻底移除**（UI Tab、Rust 枚举、Rust 函数全部删除；无保留 stub，避免误用）。请不要在这个固件项目的 issue / PR 里提出接入米坛的需求。
- **三层付费过滤（合规，绝不泄露付费内容）**：
  1. 索引解析阶段：`paid_type ∈ {paid, force_paid, ¥xxx, VIP, …}` 立即丢弃（AstroBox 源执行，标题再叠加关键词过滤兜底）；
  2. AstroBox CSV 标题二次过滤：命中 `付费 / 购买 / ¥ / 大会员 / paid / force_paid / price / money / vip` 关键词直接丢弃（避免 `paid_type` 列漏填）；
  3. 安装前 `install_from_repo` 入口再检查一次 `item.paid.is_free()`，付费条目直接拒绝，HTTP 不发起。
- **设备型号过滤**：若当前已通过 BLE 连接小米设备，按"设备名 → 型号 code"映射（Mi Band 9→n67、Redmi Watch 6→o72…）仅展示目标设备支持的资源；未连接时显示所有免费条目，方便离线浏览。
- **下载 → 缓存 → 安装** 流水线：
  - 下载占进度 0%–50%，安装占 50%–100%；
  - 流式 chunk（4–8 KB buffer）写 SD 卡 `/sdcard/astrobox/cache/<slug>_<version>.<ext>`，**不会一次性把 >4 MB 表盘装进 RAM**，保护 PSRAM；
  - 命中缓存（文件存在且大小 ≥ manifest `filesize` 的 99%）时跳过 HTTP 直接安装；
  - 未插 SD 卡时 fallback 为内存下载（≤ 16 MB 上限）。
- **HTTP 客户端**：基于 `embedded-svc::http::client::Client + EspHttpConnection + esp-tls`，**不关闭证书校验**（sdkconfig 启用 `CONFIG_MBEDTLS_CERTIFICATE_BUNDLE=y`）；15s 超时、2 次指数退避重试、每次请求独立 OS 线程 + oneshot 回 async，**绝不阻塞 Slint/Tokio 事件循环**。

### 其他
- 伪 ANCS (Apple Notification Center Service) BLE 服务（使用 KeyboardDisplay 配对能力）
- OTA 更新能力 stub（预留接口、日志为 debug 级别，生产环境不刷屏）
- ESP32-S3 PSRAM 优化
- Slint UI 渲染框架

## 硬件要求

本项目**只针对 ESP32-S3** 设计（使用了 Xtensa LX7 双核 + 八线 PSRAM + NimBLE 多连接）。其他 ESP32 型号（C3 / C6 / S2 / H2）不在支持范围内。

### 最低硬件需求（Minimum）

可以点亮固件并跑完整功能的下限：

| 项目 | 最低规格 | 说明 |
|------|----------|------|
| MCU | **ESP32-S3**（双核 Xtensa LX7，240 MHz） | 不兼容 ESP32-C3 / C6 / H2 / S2 / 初代 ESP32 |
| Flash | **8 MB** | `factory` 分区 0xE00000 ≈ 14 MB，低于 8 MB 必须改 `partitions.csv` |
| PSRAM | **Quad 4 MB （40 MHz）** | `CONFIG_SPIRAM_MODE_OCT` 若强行关闭可降级到 Quad，但 RAM 压力很大 |
| 显示屏驱动 | **SPI RGB 屏 + `mipidsi` 支持的控制器** | 官方默认 ST7789（240×320），GC9A01 圆屏引脚兼容可回切 |
| 显示屏分辨率 | **≥ 240 × 320** | UI 按 3:4 矩形布局，240×240 圆屏需回切驱动 + Slint |
| 触摸屏 | 可选，无也可启动 | 不接 CST816S 时 UI 只显示，无法交互 |
| BLE | 必须（NimBLE，5 路并发连接） | |
| 编译环境 | esp-idf **v5.3.3** + ldproxy + cmake + ninja | 详见 **编译** 章节 |

> ⚠️ 注意：如果选择 8 MB Flash / Quad PSRAM 的组合，建议：
> 1. 在 `partitions.csv` 中把 `factory` 减小到约 `0x600000`（6 MB）；
> 2. 把 `sdkconfig.defaults` 里 `CONFIG_ESPTOOLPY_FLASHSIZE_16MB=y` 改为对应值；
> 3. `save_image_rel.sh` 里 `--flash-size 16mb` 也要同步改掉。

### 推荐硬件（Recommended）

按照本仓库开箱即用的硬件配置，推荐直接买以下模块和屏幕：

| 项目 | 推荐型号 / 参数 | 备注 |
|------|----------------|------|
| SoC 模组 | **ESP32-S3-WROOM-1-N16R8V**（16 MB Flash + 8 MB Octal PSRAM, V 版本） | 与 `sdkconfig.defaults` 一一对应，零改动 |
| 屏幕 | **ST7789 240×320 方形 IPS 屏，带电容触摸（CST816S）** | 常见型号：淘宝搜"ST7789 240×320 SPI 触摸"；GC9A01 圆屏引脚兼容但需回切驱动 |
| 屏幕接口 | SPI + 6 线（CS / DC / RST / SCLK / MOSI / BL），I²C 触摸 | |
| 其他 | 1 个 LEDC 通道用于背光 PWM；USB OTG 下载口 / USB-UART 桥 | 背光默认 50% 占空比，25 kHz PWM |

**推荐开发板**：同等规格也可以选以下现成 SBC 做前期验证，再转自焊模组：
- **S3 N16R8 核心板**（淘宝常见的"ESP32-S3 N16R8 小系统板"，直接引出 SPI/I²C/LEDC 全部引脚）
- **LilyGO T-Embed / T-Display-S3 AMOLED**（需手动改 pinmux 到下方 BOM）
- **官方 ESP32-S3-DevKitC-1-N16R8V**（调试首选）

### 元器件清单（BOM）

自搭 PCB 所需的核心元器件（参考 AstroBox-NG 模块原理图）：

| 序号 | 元器件 | 型号 / 规格 | 数量 | 连接 / 引脚（默认） | 备注 |
|------|--------|------------|------|-------------------|------|
| 1 | SoC 模组 | ESP32-S3-WROOM-1-N16R8V (16MB Flash + 8MB Octal PSRAM) | 1 | — | 核心 |
| 2 | SPI 方形 LCD 模组 | ST7789, 240×320, SPI, 40 MHz | 1 | SPI2 | 见下方引脚；GC9A01 圆屏引脚兼容 |
| 3 | LCD SPI SCLK | GPIO7 | 1 | SPI2 CLK | |
| 4 | LCD SPI MOSI | GPIO6 | 1 | SPI2 D (MOSI) | LCD 侧叫 SDA / SDI |
| 5 | LCD CS | GPIO5 | 1 | 片选，推挽输出 | |
| 6 | LCD D/C | GPIO4 | 1 | 数据 / 命令选择 | |
| 7 | LCD RST | GPIO3 | 1 | 复位，低有效 | |
| 8 | LCD 背光（BL） | GPIO2 | 1 | LEDC Channel 0，25 kHz PWM | 默认 50% 亮度 |
| 9 | 电容触控芯片 | CST816S (I²C, 地址 0x15 或 0x2A) | 1 | I2C0 | |
| 10 | I²C SDA | GPIO18 | 1 | I2C0 SDA，4.7 kΩ 上拉到 3V3 | |
| 11 | I²C SCL | GPIO16 | 1 | I2C0 SCL，4.7 kΩ 上拉到 3V3 | |
| 12 | TP INT | GPIO1 | 1 | 输入，**内部上拉** | 低电平有效 |
| 13 | TP RST | GPIO0 | 1 | 推挽输出 | 复位时序见 CST816S datasheet |
| 14 | 电源 | 3.3 V LDO，≥ 500 mA 峰值 | 1 | — | ST7789 亮背光瞬时可达 ~180 mA |
| 15 | 去耦电容 | 100 nF 0402（每颗 IC 旁），10 µF 0603×2 | 若干 | — | |
| 16 | 下载口 | USB Type-C（USB OTG 直连 GPIO19/20）或 CH340K | 1 | — | `espflash` 推荐 USB OTG 方式 |
| 17 | **MicroSD 卡槽（SPI 模式，FAT32）** | TF 卡座 / MicroSD 破板，3.3V 逻辑 | 1 | SPI2（与 LCD 共享总线） | 卡规格建议 ≤ 32 GB FAT32（Cluster ≥ 8 KB）；大容量卡可格式化为 FAT32（exFAT 不支持） |
| 18 | SD SPI MISO | → ESP32 GPIO8（SPI2 MISO） | 1 | 直连卡座 DO | SD 侧常叫 DO / DAT0 |
| 19 | SD SPI CS | → ESP32 GPIO9 | 1 | 推挽输出，10 kΩ 上拉到 3V3（推荐） | 低有效 |
| 20 | 上拉电阻（SD CS） | 10 kΩ 0402（可选但推荐） | 1 | GPIO9 ↔ 3V3 | 防止启动时 SD 误应答 SPI 总线 |
| 21 | 去耦电容（SD 卡座 3V3 旁） | 100 nF 0402 + 10 µF 0603 | 1 对 | 卡座 VCC 旁 | 峰值读写电流 ~80 mA |
| 22 | LED（可选） | 红色 0805 + 1 kΩ | 1 | 任意空闲 GPIO | |

#### 引脚速查表（Pinmux Summary）

```
ESP32-S3 GPIO     → 外设
──────────────────────────────────────
GPIO0             → CST816S RST        (OUT)
GPIO1             → CST816S INT        (IN, PU)
GPIO2             → LCD BL (LEDC CH0)  (PWM, 25 kHz)
GPIO3             → LCD RST            (OUT)
GPIO4             → LCD D/C            (OUT)
GPIO5             → LCD CS             (OUT)
GPIO6             → LCD SPI MOSI/SDI   (SPI2 D)  ← MicroSD 共用：SD DAT2/CMD 方向此脚不接，仅 SDI
GPIO7             → LCD SPI SCLK       (SPI2 CLK) ← MicroSD 共用 CLK
GPIO8             → MicroSD MISO / DO  (SPI2 MISO)★ 新增
GPIO9             → MicroSD CS         (OUT, PU)  ★ 新增
GPIO16            → I2C0 SCL           (400 kHz, PU 4.7k)
GPIO18            → I2C0 SDA           (400 kHz, PU 4.7k)
GPIO19, GPIO20    → USB-OTG D-, D+     (espflash / JTAG 下载)
──────────────────────────────────────
SPI2 总线说明：
  LCD  → CS=GPIO5,   SPI mode 0, 40 MHz
  SD   → CS=GPIO9,   SPI mode 0, 20 MHz（SD 默认慢，可上探 40 MHz）
  两者共用 SCLK=GPIO7 / MOSI=GPIO6 / MISO=GPIO8；ESP-IDF SPI host 按 CS 内部串行化。
──────────────────────────────────────
```

### ST7789 方形屏适配（对应 Issue #21）

> **问：是否支持 ST7789 240×320 长方形屏幕？**（#21 @dm1366631）
>
> **答：** 已完成适配，ST7789 240×320 现为默认屏幕。三阶段工作全部落地：

| 阶段 | 内容 | 状态 |
|------|------|------|
| 1 | **驱动切换**：`src/gui/display.rs` 中 `GC9A01` → `ST7789`，`display_size(240, 320)`，`ColorInversion::Normal`，`ColorOrder::Bgr`。SPI pinmux 不变。 | ✅ 已完成 |
| 2 | **分辨率常量同步**：`slint_ui.rs` 中 `DISPLAY_HEIGHT` 240 → 320，触摸 clamp 自动跟随。 | ✅ 已完成 |
| 3 | **Slint UI 矩形重排**：`app.slint` 去掉 `border-radius: 120px` 圆形 clip，Window 改 240×320；电量/充电移至顶部状态栏，设备名居中，网速下行下方，设置按钮底部；断连呼吸点居中 y=160。 | ✅ 已完成 |

**回切 GC9A01 圆屏**：如需用回圆形屏，在 [src/gui/display.rs](file:///workspace/AstroBox-NG-Module-App-ESP32S3/src/gui/display.rs) 把 `ST7789` 改回 `GC9A01`、`display_size(240, 240)`、`ColorInversion::Inverted`、`ColorOrder::Rgb`，并把 `slint_ui.rs` 的 `DISPLAY_HEIGHT` 改回 240、`app.slint` 的 `height` 改回 240px 即可。引脚完全一致，无需改 PCB。

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

## 配置

WiFi 凭据可通过编译时环境变量或运行时 NVS 存储配置：

```bash
# 编译时设置（在 .env 或环境变量中）
# 注意：从 2026-08 起默认值已移除（修复硬编码凭据安全问题），未设置
# 时启动会打印 warn，等你通过 NVS 在运行时写入
export DEFAULT_WIFI_SSID="your_ssid"
export DEFAULT_WIFI_PASSWORD="your_password"
export MIWEAR_AUTH_KEY="0123456789abcdef0123456789abcdef"  # 32 char hex
cargo build --release
```

首次成功连接后，凭据会自动保存到 NVS，下次启动直接读取。后续可通过调用
`crate::nvs_config::save_wifi_credentials(ssid, password)` 在运行时动态写入。

此库使用 AGPL 3.0 授权

This library is licensed under AGPL 3.0

## 额外条款 / Additional Terms
根据 AGPL 3.0 所述可选附加条款，本项目额外附加署名要求，使用此项目需在遵守 AGPL 3.0 条款后额外为此项目添加署名，署名包括但不限于本项目仓库地址，作者名等。

注：附加条款以中文版为准，其他语言仅供参考！

According to the optional additional terms stated in AGPL 3.0, this project includes an additional attribution requirement. When using this project, after complying with the terms of AGPL 3.0, you must also add attribution for this project, which includes but is not limited to the project repository address, the author's name, etc.

Note: The additional terms are based on the Chinese version. Other languages are for reference only!