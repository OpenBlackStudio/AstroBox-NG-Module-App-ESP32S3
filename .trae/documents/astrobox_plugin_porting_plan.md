# AstroBox 官方 WASI 插件系统（PluginSystem）移植实施计划

> 适用范围：把 AstroBox 官方「插件系统」从 AstralSightStudios/AstroBox-NG-Module-PluginSystem 移植到当前 ESP32‑S3 N16R8 固件。目标是「宿主端能跑 `.abp` 插件」，而不是「插件作者开发体验」。

---

## 一、研究结论（Repository Research）

### 1.1 上游架构 —— 宿主（Host）和插件（Plugin）是两个世界

AstroBox 官方已经把「插件系统」拆成三个独立仓库，**分离得非常干净**：

| 仓库 | 协议 | 我们移植需要的部分 |
|------|------|-------------------|
| `AstralSightStudios/AstroBox-Plugin-WIT`（MIT） | MIT 宽松 | `main.wit` + `deps/*.wit` —— **宿主 ↔ 插件之间的字节级接口契约**，定义了 Host 要提供 16 个 import world 和 Plugin 要 export 哪些生命周期/事件回调。 |
| `AstralSightStudios/AstroBox-NG-Module-PluginSystem`（AGPL‑3.0，**带额外署名附加条款**） | **AGPL‑3.0 + 署名** | **这就是我们要移植的 HOST 侧加载器/RUNTIME**。官方 AstroBox‑NG 桌面版（Tauri）就是把它当 Rust crate 用。包含：`.abp` 包解包 → manifest 校验 → wasm 实例化 → 16 个 world import 的默认桌面实现（Fetch/UI/Timer/Dialog…）→ 事件总线。⚠️ **AGPL‑3.0 是强传染协议**，抄一行代码都可能触发源代码开放义务。 |
| `AstralSightStudios/AstroBox-NG-Plugin-Template-Rust`（MIT 模板） | MIT | 插件编译模板。输出 `.abp`（Zip：`manifest.json + icon.png + plugin.wasm`）。可作为我们的**负载测试用例**。 |
| `AstralSightStudios/AstroBox-Plugin-SDK`（npm 包，JS） | — | JavaScript 插件支持，非本计划目标（ESP32 跑 JS WASM 成本太高）。 |

### 1.2 `main.wit` 世界（已抓下来验证）

```wit
package astrobox:main;
use astrobox:psys-host/{os, transport, ui, ui-v3, clipboard, dialog, device,
                          register, event, provider-callback, queue, timer, interconnect,
                          thirdpartyapp, watchface, i18n};
use astrobox:psys-plugin/{lifecycle, event, event-v3};
world psys-world    { import (全部16个host) ; export lifecycle, plugin-event }
world psys-world-v2 { 同上 }
world psys-world-v3 { import 15个 (去掉旧 ui, 只留 ui-v3) ; export lifecycle, plugin-event-v3 }
```

**16 个 Host Import 简要职责**（全部都要 HOST 侧实现，否则插件调用就 trap）：
1. `os`：日志、env、args、时间、sleep、exit
2. `transport`：HTTP Fetch、WS
3. `ui` / `ui-v3`：声明式 UI 树 → 宿主渲染（V3 替代旧版）
4. `clipboard`：剪贴板读写文本
5. `dialog`：alert/confirm/prompt/select 对话框
6. `device`：查询/控制宿主已知的 BLE 设备（快应用、表盘列表/安装/卸载）
7. `register`：事件订阅注册（event 的前半段）
8. `event`：插件侧事件回调
9. `provider-callback`：给 provider 插件的回调
10. `queue`：消息队列
11. `timer`：setTimeout/setInterval 类定时器
12. `interconnect`：插件间互调用
13. `thirdpartyapp`：快应用安装/列表/卸载到具体 BLE 穿戴
14. `watchface`：表盘安装/列表/卸载
15. `i18n`：多语言翻译查询

**插件 export**（宿主调用，是最小面）：`lifecycle { on_load }`，`plugin-event { on_event / on_ui_event }`。

### 1.3 当前固件 AstroBox‑NG‑Module‑App‑ESP32S3 的插件基础 = 几乎为零

已验证：
- 整棵 `src/` 无 `plugin/插件/PLUGIN` 关键词。
- `local_packages.rs` 的 `LocalType` 枚举只含 `QuickApp / Watchface / ResourceBin`，**没有 `.abp` Plugin**。
- `install.rs` 只实现对 BLE 设备的 `install_quick_app` / `install_watchface`。这里的「安装」是「把二进制通过 BLE 写到手环」，**不是把插件装到 ESP32 自己的宿主 runtime 里**。
- `Cargo.toml` 无任何 WASM runtime crate；`sdkconfig.defaults` 也没开 WASM 相关的 ESP-IDF 组件。

### 1.4 结论 —— 工作量 = 巨大。不能一蹴而就，必须分阶段

**最坏风险**：一次性上「16 个 host import 全实现 + 完整事件总线 + PSRAM 爆了」→ 100% OOM / 编译不过 / 不工作。
**正确策略**：分 Phase 0–3，每 Phase 都有独立「能不能跑、能不能看日志」的交付物。

---

## 二、许可证前置问题（必须在 Phase 0 回答，否则本计划立即作废）

> ⚠️ 非技术问题，但比技术更重要。回答错了会传染 AGPL 义务。

`AstralSightStudios/AstroBox-NG-Module-PluginSystem` README 第一句就写：
> *此项目为 AstroBox 插件模块，使用 **AGPL 3.0 授权**。*
> ***AGPL 额外条款**：还必须附加署名（项目仓库地址 + 作者名）。*

AGPL‑3.0 的含义（简述，非法律意见）：
- 如果你 **直接 copy 这个仓库的 Rust 源码**（哪怕一行一行翻译）到当前固件里，**整个固件都可能要按 AGPL 开源，且所有通过网络与之交互的用户都能索取源码**。
- 如果你**不 copy 源码**，而是「只读 MIT 的 WIT 文件，自己独立实现这 16 个 host import 接口 + 自己的 `.abp` 解包/加载器 + 选一个独立的 WASM runtime（WAMR / wasm3）」，则可能被视为 **「独立实现 WIT 契约」**，AGPL 不触发。
- **`.abp` 插件包格式**（manifest.json + wasm + icon）本身是格式规范，不像代码那样受版权保护，但 `build_dist.py` 的具体魔法字节（如果有的话）要看它是 AGPL 模板还是 MIT 模板。

**本计划默认走的路径**：**"不 copy 官方 PluginSystem 源码，基于 WIT 契约独立实现"**，最大限度避免 AGPL 传染。所有 host import 的 Rust 实现都自己写，或复用已经存在的 firmware 模块（例如 timer 走 esp_idf_svc::timer、fetch 走 net_http.rs、dialog 走 Slint UI 弹层…）。

👉 **需要你批准 Phase 0 之前必须回答的 2 个问题**：

1. **Q1（许可）**：是否同意我们**不直接复制 AGPL 的 PluginSystem 代码**，而是基于 MIT 的 `AstroBox-Plugin-WIT` 独立实现 Host？（回答"不同意" = 项目从合规角度不能做，因为直接复制 AGPL 代码需要你把整个工程开源，而你刚刚还在要求把米坛抓取移到 private repo，看起来你并不想公开所有源码。）
2. **Q2（署名）**：如果未来功能进入 UI，是否接受在固件的「设置 → 关于」页加一行 `AstroBox PluginSystem © 2025 Searchstars (WIT MIT)`？——这是最低限度的合规姿态（即便没抄代码，借鉴了架构也符合惯例）。

---

## 三、文件与模块（Files and Modules Affected）

### Phase 0（不碰 firmware 源码，只做调研与依赖准备）
- `.trae/documents/astrobox_plugin_porting_plan.md`（本文件，保留）
- **新建独立 git clone 目标目录** `/workspace/upstream-repos/`（**公开仓库外**，避免把上游 AGPL 文件 `git add` 进 AstroBox‑NG）：
  - `upstream-repos/AstroBox-Plugin-WIT/`（git clone 只读 MIT 契约）
  - `upstream-repos/AstroBox-NG-Plugin-Template-Rust/`（git clone 只读模板，验证 `.abp` 生成产物的 manifest.json 字段）
  - `upstream-repos/AstroBox-NG-Module-PluginSystem/`（git clone **只读参考** —— 可人工对比 host import 语义，但绝不复制代码；如果 Q1 回答"同意独立实现"，这个目录看完就可以删以避免后续误用）

### Phase 1（MVP：宿主能 `load().call_on_load()` 成功返回）
- 新 **`src/plugin_runtime/`**（整个独立模块，feature gate `plugin_runtime`）：
  - `mod.rs`：对外暴露 `PluginSystem::new() / load(path) / unload(id) / list()` 类型 API
  - `abp_package.rs`：`.abp` Zip 解包 + `manifest.json` 解析（字段：id、name、version、author、entry、icon、permissions/world 版本）
  - `runtime.rs`：选好的 WASM runtime（见 §四）封装：`instantiate(wasm_bytes) -> Instance` + 调用 export `lifecycle.on_load`
  - `wit_host_stubs.rs`：16 个 import **全部先给 stub 实现**（`unimplemented!()` 或 `trap!()`，但不能 panic —— 改打 log::warn! 并返回一个 Result::Err 给 WASM 侧）。唯一**真正实现 1 个最小 import**：`dialog::show-info(msg)` → 写到 firmware 顶部的 install-progress-text。
- **`Cargo.toml`**：新增 optional deps：`wasm3` 或 `wamr-sys` + `zip` + feature `plugin_runtime = ["dep:wasm3"...]`。`sdkconfig.defaults`：如选 WAMR，需追加 ESP-IDF 组件配置 `CONFIG_WAMR_ENABLE_INTERPRETER=y` 等。
- **`src/local_packages.rs`**：`LocalType` 加 `Plugin(Abp)`；`classify` 加 `"abp"`；`install_local` 映射到 `plugin_runtime::load`（不是 BLE 安装）。
- **`src/gui/app.slint` + `src/gui/slint_ui.rs` + `src/main.rs`**：资源面板 Tab 0「本地(SD)」新增第 4 种类型 Tag `[插件]`。

### Phase 2（让插件"真的能做事"，16 个 import 逐个替换 stub）
按性价比从高到低挑 6–7 个先实现：
- `src/plugin_runtime/wit_host_os.rs`：log（转发到 firmware log）、时间（接 esp-idf time）、sleep（接 tokio::time）
- `src/plugin_runtime/wit_host_timer.rs`：set_timeout/set_interval（接 esp-idf 或 slint 的 Timer）
- `src/plugin_runtime/wit_host_dialog.rs`：已在 Phase 1，升级为真正的 Slint 弹层
- `src/plugin_runtime/wit_host_transport.rs`：HTTP fetch（复用 `crate::net_http`，通过 oneshot 回 WASM）
- `src/plugin_runtime/wit_host_thirdpartyapp.rs`：调用 `crate::install::install_quick_app / list_installed_quick_apps`（BLE 侧）
- `src/plugin_runtime/wit_host_watchface.rs`：调用 `crate::install::install_watchface / list_installed_watchfaces`（BLE 侧）

### Phase 3（与官方模板插件完全兼容 —— 事件 + UI）
- `src/plugin_runtime/wit_host_{register,event,queue,provider-callback}.rs`：完整事件总线，插件 `register_callback(EventType::BleConnected)` → 宿主发事件时 `on_event`
- `src/plugin_runtime/wit_host_ui_v3.rs`：把 WIT 的声明式 UI V3 tree 映射到 Slint 组件（最大一块 UI 工作）
- `src/plugin_runtime/wit_host_{clipboard,interconnect,i18n,device,register}`：剩余

### Phase 4（网络安装 .abp，复用 repo_net）
- `src/repo/astrobox_source.rs`：`RepoType` 增加 `Plugin`；`install.rs::install_from_repo` 加 Plugin 分支 → `plugin_runtime::load_from_cache`
- README：追加「如何构建 .abp / 从模板出包」一节

---

## 四、依赖与实现思路

### 4.1 WASM Runtime 选型（核心决策）

| 候选 | 协议 | 二进制尺寸（Xtensa ESP32‑S3 估算） | WASI 支持 | 备注 |
|------|------|------|------|------|
| **WAMR（WebAssembly Micro Runtime，Intel/字节跳动/三星）** | Apache 2.0 | ~250–350 KB interpreter | Preview 1，Preview 2 可用 `wasm-tools` polyfill | Espressif 官方 `esp-wasmachine` 用的就是它，ESP‑IDF 有原生组件。支持 Xtensa。AOT 可选（但 AOT 需要 Xtensa 二进制工具链，MVP 不开）。**推荐首选**。 |
| `wasm3` crate（Rust 绑定到 C wasm3） | MIT | ~120 KB | Preview 1 有限 | 体积最小，但解释慢且社区活跃度下降。纯解释。 |
| `wasmi` v0.31+ (pure Rust) | MIT/Apache 2.0 | ~400–700 KB | Preview 1 + some Preview 2 | 纯 Rust，但 binary 太大，PSRAM 吃紧。 |
| Wasmtime / Wazero | Apache 2.0 | 5–20 MB | Preview 2 | 不现实，直接排除。 |

**推荐：Phase 1 直接上 WAMR**。如果后续发现 ESP-IDF 里集成 WAMR 组件到 `esp-idf-svc` 太麻烦，再回退 wasm3。

### 4.2 WIT Host 代码生成

WIT 文件 → Rust Host Trait 的生成走官方工具链：
- `cargo install wasm-tools` + `wit-dump`，或者更常见的：
  ```
  wit-bindgen host-wasm3-rust astrobox:psys-world --world psys-world-v3 --out ./src/plugin_runtime/
  ```
  （WAMR 有自己的 bindgen crate，但原则一致：把 WIT 编译成 Rust trait，我们实现那个 trait 即完成 host import。）

Phase 1 MVP 选 **`psys-world-v3`**（新版，删了旧 ui，只保留 ui-v3），未来再加 v2/v1 兼容。

### 4.3 `.abp` 包格式（由 I18nLoader 模板推断 + build_dist.py 说明）
- Zip 压缩包
- `manifest.json` 必需字段：`id / name / version / entry(="plugin.wasm") / min_host_version / permissions`
- `icon.png` 可选，128×128 PNG
- `plugin.wasm` 必需，是 wasip1/wasi-p2 目标编译产物
- 校验步骤：① zip 完整性 → ② manifest 必需字段 → ③ entry 文件存在且是合法 WASM（magic `\0asm`）→ ④ host_version ≥ manifest.min_host_version

Phase 1 里 manifest 校验可以宽松（只要 id/name/entry 存在就允许），后面逐步收紧。

---

## 五、依赖顺序的实现步骤（Implementation Steps）

> 每一步都"独立可验证、可回退"，失败不会污染主分支代码。

### Phase 0 — 准备 + 合规确认（无代码修改写入固件源）

1. **Q1/Q2 答复**（阻塞后续所有步骤）：Q1 必须「同意独立实现」；Q2 建议同意署名。
2. 把上游 3 个仓库 clone 到 `/workspace/upstream-repos/`：
   ```bash
   cd /workspace/upstream-repos
   git clone --depth 1 https://github.com/AstralSightStudios/AstroBox-Plugin-WIT
   git clone --depth 1 https://github.com/AstralSightStudios/AstroBox-NG-Plugin-Template-Rust
   git clone --depth 1 https://github.com/AstralSightStudios/AstroBox-NG-Module-PluginSystem  # 只读参考, 看完建议删
   ```
3. 打开 `Template-Rust/scripts/build_dist.py` + `Template-Rust/manifest.json`，记录 `.abp` 的具体 manifest schema 与 zip 内文件名排布（是否带 `dist/` 前缀）。
4. 打开 `AstroBox-NG-Module-PluginSystem/src/` 目录（如果能看到）—— 只读，列出它的模块划分（`package/ runtime/ host_impls/ events/`…），为我们独立实现的同名目录命名作参考（**不抄代码，只抄结构**，避免 AGPL 风险）。
5. 输出 2 页笔记：`abp_format.md` + `host_import_semantics.md`（放在 `.trae/documents/` 里），作为后续独立实现的 specification。

### Phase 1 — MVP（最小可运行闭环）

1. 在 `Cargo.toml` 加 `[features] plugin_runtime = [...]` 与 optional `wasm3/wamr-sys`，并在 `main.rs` 用 `#[cfg(feature = "plugin_runtime")]` 把整个 `plugin_runtime` 模块关在 feature 后——**默认关闭**，保证即便插件系统炸了也不影响现有出货固件编译。
2. `src/plugin_runtime/abp_package.rs`：实现 `Package::from_file(path) -> Result<Package>` + 本文件独立 unit tests（用 Template 输出的 demo.abp，如果没 demo 就自己造一个最小 zip 测）。
3. 选 runtime（WAMR 优先），`src/plugin_runtime/runtime.rs` 实现：
   ```rust
   impl PluginRuntime {
     pub fn new(heap_bytes: usize) -> Self;
     pub fn load(&mut self, pkg: &Package) -> Result<PluginId>;
     pub fn call_on_load(&mut self, id: PluginId) -> Result<()>;
   }
   ```
4. `wit_host_stubs.rs`：生成 WIT 绑定，16 个 import 全部给 stub（返回 dummy/default 值 + log）。另外把 `dialog.show-info` 实现为真正写 `slint_ui::set_install_progress_text`。
5. `src/local_packages.rs`：加 `LocalType::Plugin`；Tab 0 列表显示 `[插件] xxx 0.1.0 (ABP)`；点击 → `plugin_runtime.load`。
6. **独立编译验证**：只开 `plugin_runtime` feature，保证 `cargo check --target xtensa-esp32s3-espidf` 通过（不需要真刷板子）。

### Phase 2 — 有用（6–7 个核心 host import 真正工作）

按顺序，每次替换 1 个 stub、写一个对应 unit test：
1. `os`（log + 时间 + sleep）
2. `timer`（set_timeout）
3. `transport` (HTTP GET 最小子集，无 body 流式)
4. `thirdpartyapp`（`list_installed_quick_apps` + `install_quick_app`）
5. `watchface`（同上）
6. `dialog`（真正的 Slint 模态弹层，Ok/Cancel）

### Phase 3 — 兼容（声明式 UI + 事件总线，工作量最大）

1. `register + event`：实现一个异步事件总线（tokio broadcast 或 mpsc），BLE 连接/断开 / Wi‑Fi up/down 都发到总线
2. `ui-v3`：把 WIT 的声明式 UI tree（Node enum：Column/Row/Button/Text/Image/List/Scroll/…）编译到 Slint 组件树
3. 剩余 import：`clipboard / interconnect / i18n / device / provider-callback / queue`

### Phase 4 — 网络源 + 官方兼容（可选，看需求）

1. AstroBox 官方源如果有 `.abp` 插件，`RepoType::Plugin` 对应索引列 restype="plugin"
2. 下载到 `/sdcard/astrobox/cache/` + 硬链接或 copy 到 `/sdcard/astrobox/plugins/`（新目录）
3. UI 增加「已装插件」子入口，显示版本号 + 卸载按钮

---

## 六、验证（Validation）

| Phase | 验证方式 | 通过条件 |
|-------|---------|----------|
| 0 | `ls upstream-repos/*` 非空 + Q1/Q2 有书面答复（本计划批准即视为答复） | 能打开 WIT main.wit + Template manifest.json，无 404 |
| 1 | **a)** `cargo check` feature on/off 均通过 | 编译无错误；plugin_runtime 模块无重复定义 |
| 1 | **b)** 手工构造最小 `demo.abp`（manifest+空 wasm 不现实，用真实模板项目 build 一个最小 on_load 只调用 dialog.info） | call_on_load 成功返回；UI 顶部 progress 文本显示 `[plugin] demo on_load: hi`；日志里能看到 stub 调用痕迹 |
| 2 | **c)** `transport::fetch("https://example.com")` 在插件里返回 OK | 日志里 fetch 结果非空 |
| 2 | **d)** `thirdpartyapp.list_installed_quick_apps(addr)` 返回 >=0 | 不 panic，且数量与 firmware 侧 list 相同 |
| 3 | **e)** 官方 Rust 模板里的 `第一个完整闭环 Dialog` 示例插件，在 ESP32 上弹框与桌面版 UI 相同 | 肉眼验证 |
| 4 | **f)** AstroBox 源列表里有 .abp 插件，点击安装后出现在「已装插件」页 | UI 能看到 |

### 全局非功能验证（Phase 1+ 每一步都要过）
- **Flash 总占用**：开了 `plugin_runtime` 后固件尺寸增幅 ≤ 600 KB（WAMR interpreter ~350 KB + 16 个 host 模块 Rust ~200 KB + bindgen glue ~50 KB）。N16R8 有 16 MB Flash，安全。
- **RAM 运行时峰值**：PSRAM 增幅 ≤ 2 MB（WAMR heap 1.5 MB + 插件 wasm image ≤ 500 KB）。
- **稳定性**：加载/卸载 10 次相同插件不崩、`plugin_runtime.list()` 数量对得上。

---

## 七、风险与降级

| 风险 | 严重度 | 处理策略 |
|------|--------|----------|
| **R1. AGPL‑3.0 传染** | 红 | 严格执行 Phase 0 Q1 "独立实现" 路径。上游 PluginSystem 的 `src/*.rs` 不做任何 copy‑paste。建议 clone 看完后立刻 `rm -rf upstream-repos/AstroBox-NG-Module-PluginSystem`。 |
| **R2. WAMR 集成到 esp-idf-svc 的复杂度** | 橙 | Phase 1 先试 1 天：如果 `embassy-wamr` / `wamr-sys` 的 Xtensa 构建脚本报错无法绕过，立刻**回退 wasm3**（体积小、纯 crate、无 ESP-IDF 组件依赖）。不能死磕 WAMR 影响交付。 |
| **R3. PSRAM 内存不足** | 橙 | Phase 1 默认 WAMR heap 设 1 MB（不是 4 MB）。Phase 3 真跑 UI 再往上调。如果仍然 OOM：**同一时刻只允许 1 个插件在运行**（"后台多插件并发"推迟到 Phase 4+）。 |
| **R4. 16 个 import 实现不完，工作量爆发** | 橙 | 每一 Phase 有明确「完成」判断，Phase 2 做完即可「插件真的有用」（能控制手环装表盘、能 fetch 天气、能弹 dialog）。Phase 3 的 UI V3 是 20% 的人用 80% 的功能，不急。 |
| **R5. `.abp` 包格式有签名校验（官方没公开）** | 黄 | Phase 1 不做签名；如果官方在 build_dist.py 里有一段 32 字节 magic tail，Phase 2 补；如果有 RSA 签名但官方没放公钥，就"官方放了我们就加，不放就只加载我们自己生成的 + 本地用户拷进去的"。 |
| **R6. 现有 quickapp / watchface 安装 BLE 传输与 plugin_runtime 抢资源** | 黄 | plugin_runtime 所有 host import 调用（除了 log/timer）必须走 `with_device_async` 现有 pipeline，与 install.rs 共享同一互斥，不会冲突。 |

---

## 八、本计划的范围声明（Do only what is planned）

- **本计划不包含**：
  - 直接复制 AGPL‑3.0 的 PluginSystem 源代码；
  - 开发「插件作者工具链」（npm SDK、JS WASM 插件支持、在线发布平台等）；
  - 插件热更新（ESP32 不支持动态 native hot reload，wasm 替换我们支持，native 不做）；
  - 米坛社区的插件源（上一轮已合规移除，不再考虑）。

---

**需要你批准的决策**：
1. 确认 Q1 = 走独立实现路径，不复制 AstroBox‑NG‑Module‑PluginSystem（AGPL‑3.0）的源码。
2. 确认 Q2 = 可以在「关于」加署名。
3. 确认按 Phase 0 → 1 → 2 顺序做，Phase 3/4 先不碰（等 Phase 2 交付后再评估是否必要）。

如批准，下一步我先执行 Phase 0：clone 上游仓库、写 `abp_format.md` 和 `host_import_semantics.md` 两个笔记。

---

## 九、Phase 0 执行记录（已完成）

> 本节为 Phase 0 实际执行结论，合并原计划的 `abp_format.md` + `host_import_semantics.md`
> 两份笔记到本计划文档（避免新增散落 .md 文件）。
>
> **决策落地**（用户跳过确认，按合规默认推进）：
> - Q1 = **同意独立实现**。AGPL 的 `AstroBox-NG-Module-PluginSystem` **未 clone**，仅参考 MIT 的
>   `AstroBox-Plugin-WIT` 与 `AstroBox-NG-Plugin-Template-Rust`。16 个 host import 全部自行实现。
> - Q2 = **同意加署名**（设置→关于页加一行）。
> - 顺序 = Phase 0 → 1，Phase 2 视运行时可行性再评估。

### 9.1 `.abp` 包格式（源自 `build_dist.py` + `manifest.json`，MIT 模板）

- **容器**：标准 ZIP，`ZIP_DEFLATED` 压缩。条目路径 **相对 `dist/`**（包内不带 `dist/` 前缀）。
- **无魔数 / 无签名**：`.abp` = 纯 zip，扩展名 `.abp`。无尾部 magic、无 RSA 签名（R5 暂不存在）。
- **打包命名**：`<safe_name>.abp`，`safe_name` 取自 manifest `name`（crate 名兜底），非法字符 `<>:"/\\|?*` → `_`。
- **必需条目**：
  - `manifest.json`（必需）
  - `<entry>.wasm`（必需，文件名 = manifest `entry` 字段，如 `astrobox_ng_plugin_template_rust.wasm`）
  - `<icon>`（可选，路径取自 manifest `icon` 字段，如 `icon.png`）
  - `additional_files` 列出的额外文件（可选）
- **manifest.json schema**：
  | 字段 | 类型 | 必需 | 含义 |
  |------|------|------|------|
  | `name` | string | ✓ | 插件显示名 |
  | `version` | string | ✓ | 语义版本，如 `1.0.0` |
  | `entry` | string | ✓ | wasm 入口文件名（包内相对路径） |
  | `icon` | string | ✗ | 图标文件相对路径 |
  | `description` | string | ✗ | 描述 |
  | `author` | string | ✗ | 作者 |
  | `website` | string | ✗ | 主页 |
  | `wasi_version` | int | ✓ | 2 → `psys-world-v2`；3 → `psys-world-v3`（推测） |
  | `api_level` | int | ✓ | 3 → 使用 `ui-v3` / `event-v3` |
  | `permissions` | string[] | ✗ | 如 `["network"]`，对应 transport 之类 |
  | `additional_files` | string[] | ✗ | 额外打包文件相对路径 |
- **加载校验链**（Phase 1 实施）：
  1. zip 完整性可读 → 2. manifest 必需字段（name/version/entry）存在 → 3. entry 文件存在于包内
     且前 4 字节是 wasm magic `\0asm` → 4. （可选）host_version ≥ manifest.min_host_version（模板无此字段，暂跳过）。

### 9.2 Host Import 语义（16 个，源自 `astrobox-psys-host.wit`，MIT）

> ⚠️ **关键技术约束**：所有 host 函数返回 `future<T>`，这是 **WASI Component Model 的 async 类型**，
> 而非普通同步返回。意味着：
> - 选用的 WASM runtime **必须支持 Component Model + async**（`future<T>`）。
> - `wasm3` 仅 WASI P1，**无法直接跑这份 WIT**（需 `wasm-tools` 把 component 拍平到 P1 module，
>   或选支持 component 的 runtime）。
> - 候选：**WAMR（`wasm_component` + `wasm_am_api`）** Apache-2.0，ESP-IDF 友好但集成复杂；
>   `wasmtime` 支持最全但 5–20 MB，ESP32 不可行。
> - **Phase 1 MVP 决策**：先实现 `WasmBackend` trait + `StubBackend`（不真正执行 wasm，
>   只记录 on_load 调用并返回 Ok），让 `.abp` 加载/校验/list/卸载 全链路在 ESP32 上跑通；
>   真正的 wasm 执行后端（WAMR/wasm-tools 拍平 + wasm3）留待有构建环境时迭代。

16 个 interface（host 侧需实现，插件侧 import 调用）：

| # | interface | 关键函数 | 对接固件模块 |
|---|-----------|---------|-------------|
| 1 | `os` | arch/hostname/locale/platform/version/astrobox-language/appearance/timezone-offset-minutes（均 `future<string>`/`future<s32>`） | 静态值 + esp-idf time |
| 2 | `transport` | send(device,data)/request(device,data)/to-json/from-json（protobuf） | `crate::transfer` |
| 3 | `clipboard` | read-text/write-text | Slint/无（stub） |
| 4 | `dialog` | show-dialog/pick-file/save-file-*/open-url | `slint_ui::set_install_progress_text`（Phase 1 最小落地点） |
| 5 | `ui` | `resource element` 声明式树 + render/render-to-text-card | Slint（Phase 3） |
| 6 | `ui-v3` | 扩展 element 类型 + animation/scroll/grid + get-render-size | Slint（Phase 3） |
| 7 | `device` | get-device-list/get-connected-device-list/disconnect-device | `crate::transfer` + ECS |
| 8 | `register` | register-transport-recv/interconnect-recv/deeplink-action/provider/card | 事件总线（Phase 3） |
| 9 | `event` | send-event(name,payload) | 事件总线 |
| 10 | `provider-callback` | resolve-provider-action/report-progress | provider 插件（Phase 3） |
| 11 | `queue` | add-resource-to-queue(quickapp/watchface/firmware) | `crate::install` |
| 12 | `timer` | set-timeout/set-interval/clear-timer | tokio::time / esp-idf timer |
| 13 | `interconnect` | send-qaic-message(device,pkg,data) | `crate::transfer::forward_app_message` |
| 14 | `thirdpartyapp` | launch-qa/get-thirdparty-app-list（`app-info` record） | `crate::install` |
| 15 | `watchface` | get-watchface-list/set-current-watchface | `crate::install` |
| 16 | `i18n` | load-json(content) | 本地 JSON（stub） |

### 9.3 Plugin Export（host 侧调用插件，最小面）

- `lifecycle.on-load()` — **同步**，加载后调用一次。Phase 1 的核心验证点。
- `event.on-event(event-type, payload) -> future<string>` — 通用事件回调
- `event.on-ui-event(event-id, event, payload) -> future<string>` / v3: `on-ui-event-v3` — UI 事件
- `event.on-ui-render(element-id) -> future` / `on-card-render(card-id) -> future` — 渲染回调

`event-type` 枚举：`plugin-message / interconnect-message / device-action / provider-action /
deeplink-action / transport-packet / timer`。

---

## 十、Phase 2 执行记录（host import 真实逻辑）

> Phase 2 范围：把 16 个 host import 中**性价比最高的 6 个**从 stub 替换为
> 真实实现。真正的 wasm 执行（Phase 3 WAMR）尚未接入，故这批实现以
> **可独立编译 / 可单测的模块函数**形式落地，Phase 3 的 wit-bindgen 生成
> trait 绑定时直接委托。

### 10.1 已落地模块

| 文件 | interface | 实现方式 | 复用固件模块 | 可单测 |
|------|-----------|---------|-------------|--------|
| `wit_host_os.rs` | `os`（info/log/sleep） | 同步，进 `HostCtx` trait 默认方法 | `log` crate + `env!` | ✅ |
| `wit_host_timer.rs` | `timer`（set_timeout/set_interval/clear） | `tokio::task::spawn_local` 调度 | tokio `time` | ✅（id/registry） |
| `wit_host_transport.rs` | `transport`（HTTP GET/POST） | async fn | `net_http` | 仅编译冒烟 |
| `wit_host_thirdpartyapp.rs` | `thirdpartyapp`（list/install/launch/uninstall） | async fn | `install` | 仅编译冒烟 |
| `wit_host_watchface.rs` | `watchface`（list/install/set/uninstall） | async fn | `install` | 仅编译冒烟 |
| `wit_host.rs`（升级） | `dialog.show-info`（Phase 1 已有） | trait 必需方法 | `slint_ui` | — |

### 10.2 关键设计决策

1. **同步 vs async interface 的 trait 归属**：
   - `os` / `dialog` 是**同步** interface → 直接进 `HostCtx` trait（`os_*` 为默认方法，
     委托 `wit_host_os`，任何 `HostCtx` 实现自动获得）。
   - `transport` / `timer` / `thirdpartyapp` / `watchface` 是 **async** interface
     （WIT 返回 `future<T>`）→ **不进 trait**，以独立 async 模块函数存在。
     原因：async trait 签名必须由 wit-bindgen 按选定 runtime 生成，手写会被
     Phase 3 覆盖且无法在无 runtime 时编译验证。

2. **timer 调度**：用 `tokio::task::spawn_local`（固件 `LocalSet` 单线程）。
   取消机制：每 timer 持 `Arc<AtomicBool>` cancel flag，`clear_timer` 置位，
   到点后检查 flag 跳过回调并自移除。全局 `ACTIVE_TIMERS: Mutex<Vec<…>>`，
   与 Phase 1 `REGISTRY` 同一模式。

3. **os.sleep 阻塞语义**：`std::thread::sleep`（ESP-IDF std 下映射 `vTaskDelay`）。
   仅 Phase 3 的 wasm 解释器 native 线程可调；**禁止**在 LocalSet 线程调用
   （会卡死事件循环）。trait 方法文档已注明。

4. **transport 隐式依赖**：`wit_host_transport` → `crate::net_http` →
   `embedded-svc/http`（经 `repo_net` / `mi_account` feature 启用）。与现有
   `net_http.rs` 同一隐式约定（`net_http` 自身未加 cfg gate），故不额外 gate。
   `plugin_runtime` 独立编译（不含 transport）时，os/timer/thirdpartyapp/watchface
   不依赖任何可选 feature。

5. **thirdpartyapp / watchface 无 feature gate**：`install.rs` 的基础
   `list_*/install_*/launch_*/uninstall_*/set_*` 函数均无 `#[cfg]`（仅
   `install_from_repo`/`slugify` 在 `repo_net` 后），故这两个 host 模块
   随 `plugin_runtime` 即可编译，不强依赖 `repo_net`。

### 10.3 Phase 2 验证状态（无 ESP 构建环境）

- **单测**（host 可跑）：`wit_host_os`（os_info 默认值 / 时区覆盖）、
  `wit_host_timer`（id 单调 / 注册表 add-remove / clear 置 flag）。
- **编译冒烟**：transport/thirdpartyapp/watchface 的 `module_compiles` 测试
  引用各 async fn，保证未被 dead-code 优化路径报错时可定位。
- **`cargo check --target xtensa-esp32s3-espidf`**：本沙箱未装 Xtensa 工具链
  （`rust-toolchain.toml` channel="esp" 未安装），全量编译验证留待本地 ESP 环境。
- Phase 1 已验证的 `.abp` 全链路（加载/校验/list/卸载）不受 Phase 2 影响。

### 10.4 待 Phase 3 接入的事项

- 真正的 wasm runtime（WAMR `wasm_component` 或 wasm-tools 拍平 + wasm3），
  让插件 `on_load` 真正执行并调用上述 host import。
- `wit-bindgen` 生成精确 host trait（含 async `future<T>` 签名），把本 Phase 的
  独立 async 模块函数委托进去。
- timer 回调目前只执行闭包；Phase 3 需把"到点"转成 `event-type::timer` 推给
  runtime 的事件分发，由 runtime 调插件 `on_event`。
- transport 的 device-to-device `send`/`request`（protobuf over BLE）与 WS。
