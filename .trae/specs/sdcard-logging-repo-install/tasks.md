# 实现任务清单：SD 卡日志 + SD 安装 + AstroBox 官方源（米坛源已按合规要求移除）

关联规格：`./spec.md`（特别注意 §八 D1′ —— 原 D1「米坛 stub」决策已被推翻，BandBBS 相关代码不得出现在公开仓库）。

---

## Task 1: SPI2 总线共享抽象 + SD 卡挂载模块 `src/sdcard.rs`

**Priority**: high
**Status**: done
**Parent AC**: AC1, AC13
**Depends on**: —
**Touch files**: `src/sdcard.rs` (new), `src/main.rs`, `src/gui/display.rs` (SPI bus extraction)

### 工作内容
1. **提取 SPI2 总线为共享资源**：当前 `gui::display::init_display_st7789` 内部创建 `SpiDriver` + `SpiDeviceDriver`。改为把 `SpiDriver` 交给一个 `Arc<Mutex<RefCell<SpiDriver<'static>>>>` 或等价结构（由 `main.rs` 创建，传给 LCD 初始化 & SD 初始化双方分别做 `SpiDeviceDriver::new(shared_driver, Some(cs), cfg)`）。
   - 注意：`esp-idf-svc` 的 `SpiDeviceDriver::new` 可复用同一个 `SpiDriver`（通过不同 `CS` pin 区分设备），这是 ESP‑IDF SPI 总线设计的默认模式，Mutex 放在 SPI 驱动层。
2. **新增 `src/sdcard.rs` 模块**：
   - `pub struct SdCard { root_path: &'static str, mounted: bool }`
   - `pub fn mount(pins: SdCardPins { miso: Gpio8, cs: Gpio9, shared_spi_driver: ... }) -> Result<Self>` 使用 `esp-idf-svc` fatfs + SpiSdCard（或 sdcmmc 驱动的 SPI 模式）；`VfsPath::new("/sdcard")` 注册。
   - `pub fn is_mounted(&self) -> bool`
   - 自动创建子目录：`/sdcard/astrobox/packages/`、`/sdcard/astrobox/cache/`、`/sdcard/logs/`。
   - 失败时 `bail!` 但不 panic，`main.rs` 里用 `warn!` 捕获，把 `SdCard` 包成 `Option<SdCard>`。

### 测试要求（TR）
- **TR 1.1 (rule)**：`mount` 成功后，`tokio::fs::metadata("/sdcard").await.is_ok() == true`。
- **TR 1.2 (rule)**：无卡环境下 `mount` 返回 `Err`，调用方不产生 panic。
- **TR 1.3 (rule)**：`mount` 后 LCD 初始化仍可写满屏黑，`main` 先 mount 再 init_lcd 都能成功（互斥性通过）。

---

## Task 2: 滚动日志 SD 后端 `src/logging.rs`

**Priority**: high
**Status**: done
**Parent AC**: AC2, AC3, AC4
**Depends on**: Task 1
**Touch files**: `src/logging.rs` (new), `src/main.rs`, `Cargo.toml`

### 工作内容
1. 新增 `FileLogger { dir: PathBuf, max_bytes: usize, max_days: usize, fail_counter: Cell<u8>, disabled: Cell<bool>, writer: Mutex<BufWriter<File>> }`。
2. 实现 `log::Log` trait 的 `enabled / log / flush`：
   - `log()` 先按 `level!` 过滤（默认 info 及以上）；
   - 格式 `[UTC ISO] LEVEL target: msg`；
   - 每次写后检查当前文件大小 ≥ 512 KB 则切分：当前文件重命名 `astrobox_YYYYMMDD_000x.log`；新建当天文件继续。
   - 清理：扫描 `/sdcard/logs/`，按 mtime 排序，总字节数 > 4 MB 或 mtime > 7 天 → delete。
3. 在 `main.rs` 中：
   - SD 挂载成功时：`log::set_boxed_logger(Box::new(CombinedLogger(SerialLogger + FileLogger)))`；
   - SD 挂载失败时：只启用 serial（目前默认已启用）。
4. 日志失败降级：`fail_counter >= 5` 就 `disabled.set(true)`，只 warn 一次 "FileLogger disabled after 5 consecutive errors"。
5. `Cargo.toml` 新增依赖（若尚未引入）：`log` 已存在，可能需要 `chrono`（`alloc` feature 模式，`no_std` 友好）+ `heapless` 或直接用 `std::time::SystemTime`。

### 测试要求（TR）
- **TR 2.1 (rule)**：连续 info! 100 条 `pattern_<N>` 后，SD 卡日志文件 grep 到 `pattern_0` 与 `pattern_99`。
- **TR 2.2 (rule)**：伪造写失败（打开 `/sdcard/logs/` 为只读文件），`disabled` 字段 5 次错误后变为 true，第 6 次 write 无错误抛出。
- **TR 2.3 (rule)**：写 8 MB 测试字节后，`du /sdcard/logs/` 总大小 ≤ 4.5 MB（切分+清理生效）。
- **TR 2.4 (rubric, 0-2, thresh 1)**：切分与清理的边界实现鲁棒性；`0 = 只靠文件大小，1 = 日期 + 大小双重，2 = 日期 + 大小 + 启动计数三重 + 异常断电重启不乱`。

---

## Task 3: 本地安装包扫描/选择 `src/local_packages.rs`

**Priority**: high
**Status**: done
**Parent AC**: AC5, AC6
**Depends on**: Task 1
**Touch files**: `src/local_packages.rs` (new), `src/install.rs`

### 工作内容
1. `pub async fn scan_packages() -> Result<Vec<LocalPackage>>`，遍历 `/sdcard/astrobox/packages/`：
   ```rust
   pub enum LocalType { QuickApp, Watchface, ResourceBin }
   pub struct LocalPackage {
     pub name: String,            // stem or manifest name
     pub path: PathBuf,
     pub size: u64,
     pub modified_at: Option<std::time::SystemTime>,
     pub r#type: LocalType,
     pub guessed_pkg_name: Option<String>, // for rpk only
   }
   ```
2. `.rpk` 包：读取 ZIP/自定义 manifest 头，取 `package_name`（如 rpk 是 ZIP，则 `manifest.json` 里读）；读不到就回退用文件名 stem。
3. `pub async fn install_local(addr: &str, lp: &LocalPackage, tx: Option<mpsc::Sender<TransferProgress>>) -> Result<()>`：
   - 读文件字节（考虑文件 ≥ 2 MB 时分块发的可能，当前 MassSystem::send_file 已支持 `Vec<u8>`，分块可暂缓先一次加载，PSRAM 应够用；但需加 warn 当 size > 3 MB）。
   - 根据 `LocalType` 调用 `install::install_quick_app` / `install_watchface` / `send_resource`。
4. `install.rs` 中补齐 `install_watchface(addr: &str, bytes: Vec<u8>)` 的公开入口（如果还缺少的话，按既有模式添加）。

### 测试要求（TR）
- **TR 3.1 (rule)**：4 个扩展名样例包放 packages 目录，scan 返回 4 条且类型全部正确。
- **TR 3.2 (rule)**：`install_local` 调用后对应 `install::*` 函数被调用（可通过 mock 或读取 ECS 中安装列表增加一项）。
- **TR 3.3 (rule)**：空目录 scan 返回空 vec，不报错。

---

## Task 4: 联网仓库通用模型 + HTTP 客户端 `src/repo/mod.rs`, `src/net_http.rs`

**Priority**: high
**Status**: done
**Parent AC**: (base for FR4/FR5/FR6)
**Depends on**: Task 2 (for logging), existing Wi‑Fi
**Touch files**: `src/repo/mod.rs` (new), `src/net_http.rs` (new), `Cargo.toml`

### 工作内容
1. `src/repo/mod.rs` 定义通用 `RepoItem`、`RepoType`、`PaidStatus`、`RepoManifest`、`RepoSource` 等类型，实现免费/设备过滤函数。
2. `src/net_http.rs` 封装 `embedded-svc::http::client::Client`：
   - `async fn get_text(url: &str) -> Result<String>`
   - `async fn get_bytes_with_progress(url: &str, cb: impl FnMut(usize, Option<usize>)) -> Result<Vec<u8>>`（chunk 写回调 + 可选 Content‑Length 总大小）
   - `async fn download_to_file(url: &str, path: &Path, cb: ...) -> Result<u64>`（直接落盘，避免大文件一次性进内存）
   - 超时 15s，自动重试 2 次，指数退避。
3. `Cargo.toml` 加 `embedded-svc`（必要 features），确认不与现有 feature 冲突；`serde`, `serde_json`, `csv`。

### 测试要求（TR）
- **TR 4.1 (rule)**：过滤一条 `paid_type=paid` 的 RepoItem 后返回 vec 不包含它。
- **TR 4.2 (rule)**：HTTP GET 到 `https://example.com`（或 github raw）200 响应，get_text 返回非空。
- **TR 4.3 (rule)**：Wi‑Fi 未连接时，`get_text` 返回明确 `Err(anyhow!("Wi‑Fi not connected"))` 不 panic。

---

## Task 5: AstroBox 官方源 `src/repo/astrobox_source.rs`

**Priority**: high
**Status**: done
**Parent AC**: AC7, AC8, AC9
**Depends on**: Task 4
**Touch files**: `src/repo/astrobox_source.rs` (new)

### 工作内容
1. 常量：
   - `const INDEX_CSV: &str = "https://raw.githubusercontent.com/AstralSightStudios/AstroBox-Repo/main/index.csv";`
   - `const MANIFEST_BASE: &str = "https://raw.githubusercontent.com/AstralSightStudios/AstroBox-Repo/main/";`
   - 可选 CDN 回退：`const INDEX_CSV_JSDELIVR: &str = "https://cdn.jsdelivr.net/gh/AstralSightStudios/AstroBox-Repo@main/index.csv";`
2. `fetch_index(filter_device: Option<&str>) -> Result<Vec<RepoItem>>`：
   - 先试 INDEX_CSV，失败用 JSDELIVR。
   - 用 `csv` crate 解析（第一行 `name,icon,cover,restype,tags,devices,path,paid_type`）。
   - `paid_type`：空串/空白 → Free，"paid" → Paid，"force_paid" → ForcePaid；**后者两者立即过滤**。
   - `devices`：按分号切分；`filter_device` 提供时必须包含该 code，否则跳过。
   - `tags`：按分号切分。
3. `fetch_manifest(item: &RepoItem) -> Result<RepoManifest>`：
   - `MANIFEST_BASE + item.manifest_path`，JSON 解析：
     - 必须字段：`repo_url`、至少一个 payload URL（`rpk_url` 或 `watchface_url` 等）；
     - 可选字段：`package_name`, `watchface_id`, `version`, `filesize`.
   - 解析逻辑参考 AstroBox‑Repo PRs 285 中公开的 manifest 模板：`{"repo_url": "...", "name": "...", "payloads": {"quickapp_rpk": {"url": "...", ...}, "watchface_mwz": {...}}}`，做宽松匹配（任一能拿到最终 URL 即可）。
4. 写单元测试（用本地 fixture CSV 验证 paid 过滤/设备过滤）。

### 测试要求（TR）
- **TR 5.1 (rule)**：fixture CSV 中 `force_paid` 记录被过滤（在返回 vec 中查不到对应 name）。
- **TR 5.2 (rule)**：device 过滤正确（n67 时 o66 条目不出现）。
- **TR 5.3 (rule)**：取 index.csv 中任意一条"免费 quickapp"（如"倒数日 快应用"），fetch_manifest 返回 payload URL 非空，HTTP HEAD 到 payload URL 返回 200。

---

## Task 6: 米坛社区源 ~~（尽力而为 stub + 最基本抓取）~~ · 已按合规要求**取消**

**Priority**: medium
**Status**: **cancelled (compliance rollback)**
**Parent AC**: ~~AC12~~（spec §十 已同步声明 AC12 被废弃，见下）
**Depends on**: Task 4
**Touch files**: ~~`src/repo/bandbbs_source.rs` (new)~~（已从公开仓库永久删除，代码备份移至仓库外 `/workspace/private-bandbbs-scraper/`）

### 取消原因（2026‑08‑19，supersedes 原 Task 6）
BandBBS 米坛社区《用户协议 / 服务条款》**明确禁止未授权的自动化抓取与爬虫访问**；
即便"只抓公开 HTML"也不被允许。项目 owner 下令立即回退，因此本 Task 6
的全部工作内容（抓取、过滤、Tab 2 UI、RepoSource::BandBBS 枚举、CSV 过滤兜底中
的米坛关键词子集）均被**从公开仓库移除**，并做如下处理：

1. `src/repo/bandbbs_source.rs` 文件**已删除**；
2. `src/repo/mod.rs` 的 `RepoSource::BandBBS` 枚举值已移除（仅保留
   `AstroBoxOfficial`）；
3. `src/gui/app.slint` 的「米坛」Tab 按钮已删除，Tab 宽度从 74px × 3 调整为
   110px × 2（只留「本地(SD)」与「AstroBox」）；
4. `src/main.rs::resource_panel_event_loop` 的 Tab=2 分支全部移除，`SourceSwitched(2)`
   输入只会 `log::warn!` 并被忽略，不发起任何 HTTP；
5. `Cargo.toml` 依赖未引入米坛专用 crate，无需改。

### 对 spec.md AC 表的对应影响
- **AC12（米坛社区 stub / 尽力而为）**：被废弃（不再视为验收项）。相应的
  `FR5 米坛社区源（Secondary，尽力而为）` 章节保留在 spec.md 中但被标记为
  "已被 §八 D1′ 取消"，避免后人重开。

---

## Task 6-extra（仅供未来若拿到 BandBBS 官方授权时重新启用）

> 若 BandBBS 未来提供官方开放 API、或你本人获得其书面授权，可从
> `/workspace/private-bandbbs-scraper/`（该目录是你本地的个人备份，
> **不得随 AstroBox-NG 公开仓库发布**）恢复实现；恢复前必须先：
> 1. 在公开 issue 中贴出 BandBBS 授权链接 / 邮件截图；
> 2. 在 spec.md §八 追加新的决策 D1″ 替代当前 D1′；
> 3. 新建独立 feature `repo_bandbbs`（默认关闭），与 `repo_net` 解耦。

---

## Task 7: 仓库到设备安装流水线 `install_from_repo`

**Priority**: high
**Status**: done
**Parent AC**: AC10, AC11
**Depends on**: Task 3, Task 5
**Touch files**: `src/install.rs`, `src/transfer.rs` (progress event)

### 工作内容
1. 在 `install.rs` 新增：
   ```rust
   pub async fn install_from_repo(
     addr: &str,
     item: &RepoItem,
     manifest: &RepoManifest,
     cache_to_sd: bool,
     progress_tx: Option<mpsc::Sender<TransferProgress>>,
   ) -> Result<()>
   ```
   - 流程：选 payload URL（按 `restype` 取 quickapp 或 watchface）→ `download_to_file`（缓存到 SD 时）或 `get_bytes_with_progress`（直接安装）→ 本地或内存 bytes → 调用 `install_quick_app` / `install_watchface`。
   - `cache_to_sd == true` 时，文件写入 `/sdcard/astrobox/cache/<slug>_<version>.<ext>`；已存在且大小 / SHA（可简单 size 匹配）一致时跳过下载直接用缓存。
   - 进度分两段：下载 0‑50%，安装 50‑100%（通过 `TransferProgress` 映射）。

### 测试要求（TR）
- **TR 7.1 (rule)**：免费 watchface + cache_to_sd=false 安装，list_installed_watchfaces 新增一条名字匹配。
- **TR 7.2 (rule)**：cache_to_sd=true 后再执行一次 install_from_repo，第二次不再下载 HTTP（通过缓存文件存在 & 日志断言"cache hit"）。
- **TR 7.3 (rule)**：无网时 install_from_repo 返回 Err，不影响已安装列表。

---

## Task 8: Slint UI 新增资源入口（最小实现）`src/gui/app.slint` + Rust 胶水

**Priority**: medium
**Status**: done
**Parent AC**: FR7
**Depends on**: Task 3, Task 5, Task 7
**Touch files**: `src/gui/app.slint`, `src/gui/slint_ui.rs` (回调注册), `src/main.rs` (路由)

### 工作内容
1. `app.slint` 在 `connected` 分支底部：
   - 长按 ⚙ 按钮（≥500ms）触发 → 显示一个全屏面板 "安装资源"；
   - 三个列表项：`[本地包 (SD)] [AstroBox 源] [米坛源]`；
   - 选择源后显示前 5 条 + "上一页/下一页"按钮；
   - 选中一条后弹出"安装确认"文本（显示名字/大小），触发 `install_from_repo`。
2. `slint_ui.rs` 注册回调（长按按钮回调、点击列表项回调、确认按钮回调），通过 `mpsc::channel` 把事件转给 `main.rs` 的 async task 执行（UI 线程不阻塞）。
3. 安装进度回写 UI 顶部 Text "安装 倒数日 24%…"。

### 测试要求（TR）
- **TR 8.1 (rule)**：长按 ⚙ 按钮后 3 个源 Tab 能显示，选择"本地包"时若 SD 有 4 个样例包，UI 显示文件名列表（至少第 1 条可见）。
- **TR 8.2 (rule)**：选择"AstroBox 源"时至少渲染 1 条非付费记录（非空）。
- **TR 8.3 (rubric, 0-2, thresh 1)**：UI 响应性；`0 = 下安装时整个 UI 卡死 > 2s，1 = 安装时界面略有掉帧但不阻塞，2 = 安装时齿轮按钮仍有呼吸动画，进度条平滑`。

---

## Task 9: main.rs 初始化编排 + README 更新

**Priority**: medium
**Status**: done
**Parent AC**: (integration)
**Depends on**: Task 1-8 全部
**Touch files**: `src/main.rs`, `src/lib.rs?` (no bin), `README.md` (新章节: 硬件 MicroSD 接线 / 功能列表说明)

### 工作内容
1. `main.rs` 初始化顺序调整为：
   ```
   pins → NVS → Wi-Fi → SdCard::mount (SPI2 shared driver) → logging init → LCD init via shared SPI → Touch → Slint platform → Spawn BLE/miwear → Spawn repo_install event listener → Slint run forever
   ```
2. `README.md`：
   - 在"元器件清单（BOM）" + "引脚速查"增加 MicroSD SPI 接线（GPIO8 MISO / GPIO9 CS / 3V3 / GND）。
   - 在"功能特性"加：
     - MicroSD FAT32 滚动日志（4 MB 上限）
     - 从 SD 卡安装快应用/表盘/资源二进制
     - 联网接入 AstroBox 官方源 + 米坛社区源（自动过滤付费）
     - 缓存下载包到 SD 卡以离线重安装
3. 把 sdkconfig.defaults / Cargo.toml features 中新增的 sdmmc / fatfs / http 配置（若有 ESP-IDF kconfig 需求）写注释同步到 README 中。

### 测试要求（TR）
- **TR 9.1 (rule)**：按 README 接线后启动，10 秒内 UI 正常显示，SD 挂载日志不重复刷屏。
- **TR 9.2 (rule)**：README 新增 BOM 两行与 pinmux 新增两行正确（与代码 GPIO 常量一致）。

---

## Task 10: 文档化"开放问题"默认决策，代码注释

**Priority**: low
**Status**: done
**Parent AC**: NFRs
**Depends on**: Task 9
**Touch files**: 各模块文件头部 doc 注释 + `spec.md` (append decisions)

### 工作内容
1. 对 spec § 七"开放问题"在实现时写下默认决策：
   - OQ1（米坛接口）→ stub 文件 `bandbbs_source.rs` 中注释说明抓不到的原因与未来可接入位置。
   - OQ2（NTP）→ Wi‑Fi 连接成功后用 `sntp::init` + `pool.ntp.org`，失败则日志文件名改用 `astrobox_epoch<secs>.log`。
   - OQ3（SD 上限）→ 下载前查询剩余空间（`statvfs`），< 32 MB 即 warn，< 8 MB 拒绝写入缓存/安装。
   - OQ4（目录结构）→ 按 §FR3 的 `/sdcard/astrobox/{packages,cache}` + `/sdcard/logs/`。
2. 每个新模块文件顶部 doc 注释说明：用途、对 AC 的对应、错误降级策略。

### 测试要求（TR）
- **TR 10.1 (rule)**：`cargo doc`（或至少 rustfmt）后没有 `missing_docs` lint 报错（若开启）。
