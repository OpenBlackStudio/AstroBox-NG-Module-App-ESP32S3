# 规格说明：SD 卡日志输出 + MicroSD 安装快应用/表盘 + 联网接入 AstroBox 官方源（过滤付费资源；米坛社区源已按 BandBBS ToS 合规移除）

> 版本：1.1（2026‑08‑19：§八 D1′ 回退米坛接入）
> 目标仓库：`OpenBlackStudio/AstroBox-NG-Module-App-ESP32S3`（当前代码在 `/workspace/AstroBox-NG-Module-App-ESP32S3`）

## 一、背景与用户

当前 ESP32‑S3 固件可通过 BLE 连接小米/Redmi Vela 穿戴设备并进行 `list_* / install_*` 等管理，但有三个明显缺失：

1. **调试只能靠 RTT / USB 串口看日志**，无法保存历史日志，出问题回溯困难。
2. **安装快应用/表盘必须通过 WebAssembly UI（AppWasm 或 OronBox）**，需要身边有浏览器/PC；希望把 `.rpk` / `.mwz` / `.face` 等安装包拷进 MicroSD，无需联网即可安装。
3. **已接入 Wi‑Fi** 但没有利用网络下载社区资源。原规划同时接入米坛社区（BandBBS）与 AstroBox 官方源，但 **BandBBS 用户协议 / 服务条款明确禁止未授权的自动化抓取**（见 §八 D1′），故本规格 v1.1 起**仅保留 AstroBox 官方源**作为联网接入对象。

**直接用户**：ESP32‑S3 N16R8 + ST7789（240×320）核心板 / 自搭 PCB 用户，手里有小米手环 9/10、Redmi Watch 5/6、小米 Watch S3/S4/S5 等受支持设备。

## 二、目标（Goals）

### G1. 日志输出
- 系统 `log`（`info!/warn!/error!/debug!`）同时写入 MicroSD 卡的滚动日志文件，保留最近 N 天/最近 M MB。
- SD 卡未插入或写入失败时，**不影响主流程**（静默降级为只打到串口）。

### G2. MicroSD 卡本地安装
- 可在 SD 卡挂载目录 `/sdcard` 下扫描特定扩展名（`.rpk` / `.mwz` / `.face` / `.bin` 等），展示文件名、大小、修改时间；
- 选中某文件后，按扩展名自动映射到 `install_quick_app` / `install_watchface` 等已有设备安装流程，无需二次解析。

### G3. 联网接入 AstroBox 官方源（仅）
- **AstroBox 官方源**：GET `https://raw.githubusercontent.com/AstralSightStudios/AstroBox-Repo/main/index.csv`（有 `index_v2.csv` 作为增强索引），解析 CSV 得到 `name / icon / cover / restype / tags / devices / path / paid_type`。
- ~~**米坛社区（BandBBS）**：Discuz!X + "资源商城"API ……作为次级源接入。~~（v1.1 起删除，见 §八 D1′）
- **付费过滤**：索引项 `paid_type == "paid"` 或 `"force_paid"`，或标题命中 `PaidKeywordFilter::default_generic()` 的项目**必须过滤掉**，绝不进入可用列表与下载路径。
- 根据当前连接设备的 `device_model`（如 `n67`, `o66`）只展示对应型号的资源。

### G4. 网络安装
- 选定某条索引后，用 HTTP(S) GET 下载：
  - AstroBox 官方源中每项的 `path` 形如 `searchstars/hyperbili.json`，需先按 CSV base URL 解析清单 JSON 再取最终资源二进制 URL（manifest 通常有 `repo_url`、`payloads:{rpk,watchface,...}` 等字段，参考 `AstroBox-Repo` 已提交 PR 中的 manifest 示例）。
  - 米坛源同理，manifest → 下载 URL。~~（v1.1 起删除米坛源，仅保留 AstroBox manifest 解析；米坛相关句子保留仅为 audit trail。）~~
- 下载后可**直接安装**到已连接 BLE 设备，或**缓存到 SD 卡**以便以后离线安装。

## 三、非目标（Non‑Goals）

- ❌ 不实现完整文件管理器（不需要新建目录、重命名、删除），只做扫描/读取/写入缓存/安装包落盘。
- ❌ ~~不接米坛社区的账号登录、回复、点赞、充值、大会员等"非公开资源区"功能。~~（§八 D1′ 已更严格：**公开仓库不得出现任何米坛抓取代码**，无论是否涉及登录。本条划掉以避免暗示"未来还有非登录版可用"。）
- ❌ 不实现付费资源绕过或破解下载。
- ❌ 不做 SD 卡热插拔的轮询中断线硬件处理（用户自己接 GPIO 做 CD 可扩展，本版默认启动时挂载）。
- ❌ 不做 OTA 固件从 SD 卡更新（`src/ota.rs` 现有的 OTA stub 不变）。

## 四、功能需求（Functional Requirements）

### FR1. SD 卡挂载
- SPI 模式：`SPI2`（LCD 已占用 SCLK=GPIO7, MOSI=GPIO6），新增 MISO=GPIO8、SD_CS=GPIO9（3.3V）；LCD CS=GPIO5 保持独立，共享 SPI2 总线必须互斥上锁。
- 使用 `esp-idf-svc` 中 `Fatfs`/`SpiSdCard` + VFS 注册到 `/sdcard`。
- 启动时尝试挂载：
  - 成功：info 日志；
  - 失败：warn 日志，降级模式（G2/G4 不可用，G1 只输出到串口）。

### FR2. 滚动日志写入 SD 卡（`log` crate 全局 logger）
- 在现有 `log`（crate log 0.4）后端基础上，新增一个 `FileLogger`：
  - 文件名格式：`/sdcard/logs/astrobox_YYYYMMDD.log`；
  - 单文件 ≥ 512 KB 时切分；
  - 保留总大小 ≤ 4 MB，保留最近 7 天；
  - 写失败连续 ≥ 5 次自动降级（关闭 SD 写，warn 一次），不再反复重试影响性能。
- 行格式：`[YYYY-MM-DDTHH:mm:ssZ] LEVEL module::path: message`。
- 与 USB 串口输出并行，互不干扰。

### FR3. 本地包扫描与安装
- 扫描目录：`/sdcard/astrobox/packages/`（不存在则自动创建）。
- 识别扩展名：
  | 扩展名 | 映射 |
  |--------|------|
  | `.rpk` | 快应用 → `install_quick_app(addr, pkg_name, bytes)`（pkg_name 从 rpk 头 `manifest.json` 读；读不到则取文件名 stem 做兜底名） |
  | `.mwz` | 表盘 → `install_watchface(addr, bytes)` |
  | `.face` | 表盘 → 同上 |
  | `.bin` | 二进制资源 → `send_resource(addr, bytes)` 走 MassComponent |
- 结果以 API 返回给 UI：`(file_name, size, modified_at, type)`；安装触发时调用已有 `install::*` 接口，进度复用 `transfer::TransferProgress`。

### FR4. AstroBox 官方源接入（Primary）
- `src/repo/astrobox_source.rs`：
  - `async fn fetch_index(cache_dir: Option<&Path>) -> Result<Vec<RepoItem>>`
  - 通过 HTTPS GET 取 `index.csv`（超时 15s，失败自动重试 2 次 + 退避）；可写入缓存。
  - `RepoItem` 字段：
    ```rust
    pub struct RepoItem {
      pub name: String,
      pub icon_url: String,
      pub cover_url: String,
      pub restype: RepoType,    // quickapp | watchface
      pub tags: Vec<String>,
      pub devices: Vec<String>, // e.g. "n67", "o66"
      pub manifest_path: String, // "searchstars/hyperbili.json"
      pub paid: PaidStatus,      // Free | Paid | ForcePaid
    }
    ```
  - **过滤**：`paid != Free` 直接 discard；按当前连接设备的 device code 过滤 `devices`。
  - `async fn fetch_manifest(base_manifest_url: &str, manifest_path: &str) -> Result<RepoManifest>`。
    - `RepoManifest` 至少含：`repo_url`、`download_rpk_url` / `download_watchface_url`、`package_name`、`watchface_id`、`version`、`filesize`。
    - 解析以已公开的 AstroBox‑Repo 提交 PR 中 manifest 结构为准（每个仓库下的 `*.json`）。
  - `async fn download_payload(url: &str, progress_cb: ...) -> Result<Vec<u8>>`，下载后可直接安装或写 SD。

### FR5. ~~米坛社区源（Secondary，尽力而为）~~ · 已被 §八 D1′ **合规取消**

> **取消原因**：BandBBS 米坛社区《用户协议 / 服务条款》**明确禁止未授权的自动化抓取
> 与爬虫访问**；即便"只抓公开 HTML"也不被允许。项目 owner 于 2026‑08‑19 下令
> 立即回退（保留该段仅用于审计追踪，不再视为功能需求）。
>
> 原 FR5 内容（作废保留）：
> - `src/repo/bandbbs_source.rs`：尝试接入其公开资源索引页（`bandbbs.com` /
>   资源分类页 HTML）或可用 REST；若需要登录 / captcha / 大会员权限即放弃，
>   return `Ok(vec![])` 并 warn 一次；同样执行"付费过滤 + 设备型号过滤"；
>   为 UI 合并源：`RepoItem.source = AstroBoxOfficial | BandBBS`。

### FR6. 网络安装通道
- 新增：`src/install.rs` 暴露
  ```rust
  pub async fn install_from_repo(addr: &str, item: &RepoItem, cache_to_sd: bool) -> Result<()>
  ```
  内含：fetch manifest → download payload → cache（可选）→ 调用 `install_quick_app` 或 `install_watchface`。
- 进度事件走 `TransferProgress`，现有 UI / 调试文本可复用。

### FR7. UI 入口（app.slint 最小改动，可后续加强）
- 已连通状态新增 **Settings 齿轮长按 → 资源菜单** 两层入口：
  - "本地包 (SD)"
  - "AstroBox 源"
  - ~~"米坛源 (若可用)"~~（已删除，见 §八 D1′）
- 列表样式可先复用当前字体宽度，滚动方向纵向；不苛求复杂滚动容器，先做"首页 5 条 + 下一页按钮"。
- 显示图标/封面图可不做（需联网解码 PNG 资源消耗 RAM），先显示文本名与类型 Tag。

## 五、非功能需求（Non‑Functional Requirements）

### NFR1. 稳定性
- SD 卡不存在/损坏：所有 SD 写入返回 `Result`，调用方统一 warn 降级，**绝不 panic**。
- 源抓取失败：fallback 不阻塞主 UI / BLE / 触摸。
- SPI 总线：LCD（写）与 SD（读/写）共用 SPI2，必须用 `Mutex<Spi2Bus>` 串行化。若 LCD 正在刷新被锁住，触摸事件最多延迟 ≤ 1 帧。

### NFR2. 内存占用
- 大文件下载：**流式**（chunk 写 SD / 固定 4 KB 缓冲），不得一次性把 >4 MB 表盘装进 RAM。
- HTTP 客户端：PSRAM heap，下载缓冲不超过 16 KB。
- 索引 CSV ≤ 500 行时 ≤ 64 KB。

### NFR3. 可移植 / 可编译
- Rust edition 2021，`rust-version ≥ 1.77`。
- 只新增 `esp-idf-svc` 已有 feature（sdmmc fatfs http）、`embedded-svc` 的 `http/client`、`csv`, `serde`, `serde_json`、`tinyvec`/`heapless` 等成熟 crate；不引入阻塞型同步 HTTP client（必须用 `embedded-svc::http::client::Client` async 兼容封装或 `esp-idf-svc` 提供的 C‑level HTTP 封装）。

### NFR4. 安全
- HTTPS 证书校验：使用 `esp-tls` 默认的 trusted‑root bundle（不要禁用）。
- 米坛社区接入已被 D1′ 永久取消；**严禁**在公开仓库的任何代码 / 文档 / issue / commit message 中
  出现米坛账号、Cookie、Token、登录墙破解参数。

### NFR5. 合规（付费过滤）
- **绝对不**向用户展示 `paid_type == paid/force_paid`（或标题命中通用付费关键词）的 AstroBox 官方源条目。
- 必须在"读 CSV / 解析 manifest / 下载 URL 生成"三个阶段都做 paid 检查（米坛相关分支已被删除，不再执行）。

## 六、约束 / 依赖 / 假设

### 约束
- 当前工程依赖 `corelib = { path = "../core" }`，但沙盒里 `core` 不存在；代码改动必须通过"类型级别检查"，不做本地实机验证。
- 单线程 Tokio runtime（`rt-multi-thread` feature 已开启但实际以 `#[tokio::main]` 的默认配置跑；若 SD/HTTP 阻塞调用可能阻塞事件循环，必须使用之前已做的 `wifi-wd` 模式——放到单独 OS 线程，async 层走 oneshot/mpsc）。

### 依赖
- 新增依赖（需在 `Cargo.toml` 加）：
  - `embedded-svc`（features: `http`、`utils`）
  - `esp-idf-svc`（已有，features：`experimental` 默认开，需确认 `fatfs`/`sdmmc` features）
  - `csv`
  - `serde` + `serde_json`
  - `chrono` 或 `heapless::String` 组合（ESP32 上拿 UTC 时间需 NTP 同步；NTP 失败用 epoch 秒数做文件名也行）

### 假设
- 核心板已正确接好 MicroSD SPI 模组到 GPIO8/MISO + GPIO9/CS + 3V3 + GND；屏幕模组同样接在 SPI2 上。
- 用户已经配置过 Wi‑Fi SSID/密码（NVS）；若没连上网，则源相关功能返回"未连接网络"友好提示。
- `AstroBox-Repo` index.csv 的 raw 直链长期稳定可用；如果 GitHub Raw 不稳定，可切换到 `jsdelivr` CDN mirror 作为回退。

## 七、开放问题（Open Questions）

1. ~~**米坛 REST 不可用性**：米坛是封闭论坛，资源下载可能要登录。若代码接入发现拿不到开放 API，**是否接受"只接 AstroBox 官方源，米坛源留 stub 文件 + UI 标 coming soon"**？~~
   - **自答（2026‑08‑19）**：问题已失效。BandBBS ToS 禁止未授权抓取 → 不仅不能留 stub，**连空文件都不能留**；见 §八 D1′。公开仓库执行"彻底移除"策略。
2. **是否需要 NTP**：日志文件名需日期，`NTP` 联网后一次同步；无网时文件名用单调计数器或 epoch 秒，可接受吗？
3. **资源缓存大小上限**：SD 卡一般 >=8 GB FAT32，4 MB 日志上限 + 下载包不设硬上限（由 SD 剩余空间检查失败降级），对吗？
4. **SD 卡目录结构**：`/sdcard/astrobox/packages/`（用户放安装包）+ `/sdcard/astrobox/cache/`（下载缓存）+ `/sdcard/logs/`（日志），可以吗？

---

## 八、开放问题最终决策（Implementation Decisions）

> 本节记录实现阶段（`tasks.md` 中各任务落地时）针对 §七 4 个 OQ 做出的决策。
> 如果决策被后续合规检查推翻，必须**保留原决策文本**（通过 strike 或者追加 Superseded 段），
> 而不是直接删除，保证可追溯。

### D1. OQ1 → 米坛接口（*原决策已被 Superseded by D1′，见下*）

> ~~**决策**：在 `src/repo/bandbbs_source.rs` 实现"尽力而为的公开抓取"……~~
>
> **Superseded 原因（2026‑08‑19）**：BandBBS 米坛社区服务条款 / 用户协议
> **明确禁止未授权的自动化抓取与爬虫访问**。项目 owner 已下令：
> 所有与 BandBBS 相关的爬取代码必须**从公开仓库立即移除**，
> 迁移到私有仓库保存或直接销毁。因此本文件 D1 原决策被废弃。

### D1′. OQ1 → 米坛接口（合规回退版 · 2026‑08‑19 生效）

**新决策**：
1. **公开仓库中不再保留任何 BandBBS 相关代码**：
   - 永久删除 `src/repo/bandbbs_source.rs`；
   - `RepoSource` 枚举仅保留 `AstroBoxOfficial`，永不增加 `BandBBS` 值；
   - UI `app.slint` 资源面板仅保留 Tab 0「本地(SD)」与 Tab 1「AstroBox」；
   - `main.rs::resource_panel_event_loop` 的 Tab 枚举上限从 `0..=2` 缩到 `0..=1`，Tab=2 输入被 log::warn! 后丢弃，不再触发任何 HTTP；
   - README 中删除所有"米坛源即将接入 / 已接入 / stub 形式存在"的宣称，改为一段明确声明：
     *"BandBBS 因合规移除，请不要再提接入米坛的 issue / PR。"*
2. **代码存档（仅作为用户个人备份，不可随公开仓库传播）**：
   - 被删除的 `bandbbs_source.rs` 原样迁移到公开仓库**之外**的目录
     `/workspace/private-bandbbs-scraper/`，并附一份 `README.md` 说明
     *"只能由本人 `git init` 成**私密**仓库，禁止 push 到 public。"*
   - TRAE 当前环境无 GitHub/Gitee API，无法替你 `gh repo create --private`；
     该目录的 **private 仓库创建必须由你本人在浏览器里手动执行**（D1′ README 中附命令示例）。
3. **禁止**的未来行为（违反即需回退）：
   - 在 AstroBox‑NG 公开仓库新建任何调用 `bandbbs.com`、`www.bandbbs.com`、
     其 CDN / API 反代域名的 Rust 模块；
   - 在 Slint UI / README / issue 中承诺"未来支持米坛下载"；
   - 在公开 issue / PR / commit message 中粘贴米坛 Cookie / Token /
     登录墙破解参数。

**理由**：合规优先。BandBBS ToS 禁止未授权自动化抓取，违反会导致 owner / 项目
共同法律风险；D1 即便"只抓公开 HTML"也无法改变这个事实，因此按 owner 指令
从公开仓库彻底移除，不留 stub。

### D2. OQ2 → NTP：best-effort SNTP（pool.ntp.org），失败用 epoch 秒 + 启动计数兜底
**决策**：
- `sdkconfig.defaults` 已开启：
  - `CONFIG_LWIP_SNTP_ENABLED=y`
  - `CONFIG_SNTP_SERVER_NAME="pool.ntp.org"`
- `src/main.rs::spawn_sntp_init_best_effort()` 在独立 OS 线程中 `sntp_setoperatingmode(POLL) + sntp_init()`；`catch_unwind` + `std::panic` 静默失败；等待 200 ms 给网络 up 后发起，**不阻塞 Tokio runloop**。
- 日志时间戳：`src/logging.rs::now_iso_utc()` 先 `SystemTime::now() → duration_since(UNIX_EPOCH)`，成功走 `YYYY-MM-DDTHH:mm:ssZ`；**失败**（NTP 未到位、1970 年前）退化到字符串 `epoch-<秒数>`，秒数来自 `esp_timer_get_time()/1_000_000`（不会为负）。
- 日志文件名：`astrobox_YYYYMMDD_NNNN.log`，YYYYMMDD 拿不到时使用 `unknown-date` 常量，靠后半段 `NNNN` 自增索引仍能切分，不会文件名冲突。

**理由**：避免对 NTP 成功做硬依赖；离线（无外网实验室环境）也能产线使用，日志至少保存在 epoch 秒粒度，后续接网可回正。

### D3. OQ3 → SD 卡空间上限：日志 4 MB 固定 + 下载缓存 / 安装包按剩余空间动态拒绝
**决策**：
- **日志**：保持 §FR2 原规格（`MAX_SINGLE_LOG_BYTES = 512 KB`, `MAX_TOTAL_LOG_BYTES = 4 MB`, `MAX_LOG_DAYS = 7`）。`FileLogger::purge_if_needed()` 在创建首文件时 & 每次 rotate 时都执行一遍：按 mtime 新→旧累加，**超过 4 MB 或 7 天**就从最旧的开始删（`4.5 MB` 容忍余量对应 AC3）。
- **下载缓存 / 安装包写入**：
  - `FREE_WARN_BYTES = 32 MB`：写入前 `statvfs("/sdcard")` < 32 MB 时 UI 文本 warn 一次，继续允许写入（小表盘仍可下）；
  - `FREE_DENY_BYTES = 8 MB`：< 8 MB 直接返回 `Err`，不写缓存 / 不落盘安装包，避免 FAT 表写坏。
  - 包读取内存态也限制 `16 MB` 上限，超过直接 `bail!`（对应 `local_packages.rs:install_local` 与 `net_http.rs:blocking_http_get_once` 的 16 MB 阈值）。
- 不设全卡下载包总上限：SD 一般 ≥ 8 GB 足够，由用户自行管理 `/sdcard/astrobox/packages/` 内容（非目标：不做文件管理器）。

**理由**：4 MB 日志 = 约 8 片 512 KB，基本覆盖 1–2 天排障周期；下载缓存按剩余空间拒绝比写死上限更灵活（不同容量卡自适应）。

### D4. OQ4 → 目录结构：严格固定 `/sdcard/astrobox/{packages,cache} + /sdcard/logs/`，启动时自动创建
**决策**：
```
/sdcard
  ├─ logs/
  │   └─ astrobox_YYYYMMDD_NNNN.log       (滚动)
  └─ astrobox/
      ├─ packages/                        ← 用户手动放安装包（.rpk/.mwz/.face/.bin）
      └─ cache/                           ← 网络安装落盘缓存，命中时跳过 HTTP
```
- `src/sdcard.rs` 导出常量 `SDCARD_ROOT = "/sdcard"`、`DIR_LOGS`、`DIR_PACKAGES`、`DIR_CACHE`；`SdCard::mount()` 成功后统一 `create_dir_all`，`EEXIST` 不是错误（`AlreadyExists` match 掉）。
- 所有模块不写死路径字面量，一律走上述常量或 `sd_root.join("astrobox/packages")`，保证未来改 mount 根只改一处。
- 目录权限依赖 FATFS 默认 RW（FAT 无 POSIX owner），不做 `chmod`。

**理由**：结构清晰、包与缓存分离，用户把包放到一个确定位置即可；启动自动创建，无需用户手动 mkdir。

---

## 九、验收标准（Acceptance Criteria）

所有 AC 类型为 `rule` 或 `rubric`。

| ID | 类型 | 内容 | 通过条件 |
|----|------|------|---------|
| AC1 | rule | SD 卡挂载成功 | 启动日志包含 `Mounted /sdcard (FAT)` 或 SD 卡不存在时 warn `No SD card, fs features disabled`，均不 panic。 |
| AC2 | rule | 日志同时输出到 SD 卡 | 产生 ≥100 行 info! 日志后，在 `/sdcard/logs/astrobox_*.log` grep 可匹配到对应的关键字段。 |
| AC3 | rule | 日志滚动策略生效 | 写满 512 KB × N 文件后，目录总大小 ≤ 4.5 MB（4 MB 上限 + 1 个分片余量）。 |
| AC4 | rule | SD 卡故障降级 | 运行中强制移除 SD（只读 / 拔出），固件不崩；恢复插入后（至少下次启动）重新可用。 |
| AC5 | rule | 本地包扫描：rpk/mwz/face/bin 四类文件能列出 | 拷 4 个扩展名的样例包到 `/sdcard/astrobox/packages/`，API 返回 4 条记录，类型与扩展名映射一致。 |
| AC6 | rule | 本地安装调用 install_* | 选一个 MWZ 文件触发安装，`install_watchface` 收到 `data == 文件字节`，`TransferProgress` 从 0% → 100%。 |
| AC7 | rule | AstroBox 源 index.csv 解析 + paid 过滤 | index.csv 至少包含 `"paid"` 或 `"force_paid"` 的行；返回的 `Vec<RepoItem>` 中**不存在**这些行（可通过对应 name 做断言）。 |
| AC8 | rule | 设备型号过滤 | 构造含 devices=`n67` 与 `o66` 的两条记录；连接 n67 设备时只看到 n67 行。 |
| AC9 | rule | manifest 解析 → 下载 URL 正确 | 取 index.csv 中一条免费 quickapp 记录（如倒数日），`fetch_manifest` 返回结果的 `rpk_url` 字段非空且 http head 200。 |
| AC10 | rule | 网络安装成功 | 选一条免费 watchface，`install_from_repo(addr, &item, false)` 完成后 `list_installed_watchfaces` 能查到对应名字。 |
| AC11 | rule | 缓存到 SD 生效 | `install_from_repo(addr, &item, true)` 执行后，`/sdcard/astrobox/cache/<sanitized_name>.mwz` 文件存在且大小 ≥ manifest 声明大小 -1% / +5% 网络边界。 |
| AC12 | rule | ~~米坛社区 stub / 尽力而为~~ · **已废弃**（§八 D1′ 合规回退） | ~~若无登录无法抓取公开索引，`bandbbs_source::fetch_index()` 返回空 `Vec` + 一次 warn，绝不 panic 或无限重试。~~（由于 BandBBS ToS 禁止未授权抓取，相关实现已从公开仓库永久删除；该 AC 不再计入验收，保留仅为审计编号连续性。） |
| AC13 | rule | SPI LCD + SD 并发安全 | LCD 持续刷新（60 Hz Slint render）+ SD 卡持续写入日志 5 min，两者都无乱码/写错误。 |
| AC14 | rubric | 内存占用（0‑2） | `0=峰值 > 4 MB PSRAM，1=峰值在 2‑4 MB，2=峰值 ≤ 2 MB PSRAM`；≥1 才通过。 |
| AC15 | rubric | 功能清晰度与代码结构（0‑2） | `0=三处功能耦合到 main.rs，1=分模块合理但注释少，2=独立 sdcard/log/repo 模块，对外 API 清晰带 doc 注释`；≥1 通过。 |
| AC16 | rule | **合规红线**（新增 · 2026‑08‑19） | 主仓库中不得出现：① `RepoSource::BandBBS` 枚举值；② `src/repo/bandbbs_source.rs` 文件；③ `app.slint` 里的「米坛」Tab 按钮；④ `Cargo.toml` / `*.rs` 中引用 `bandbbs.com` 域名的字符串。全部 4 项满足即通过。 |
