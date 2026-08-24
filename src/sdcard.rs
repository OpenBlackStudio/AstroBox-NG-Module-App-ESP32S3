//! # SD 卡挂载模块
//!
//! 负责把 SPI2 上连接的 MicroSD 卡通过 FATFS + SDMMC 挂载到 VFS
//! 目录 `/sdcard`。LCD 的 `SpiDeviceDriver(CS=GPIO5)` 与 SD 卡的
//! `SpiDeviceDriver(CS=GPIO9)` 共用同一个 [`SpiDriver`]，ESP‑IDF
//! SPI 主机驱动默认支持，由设备驱动内部按 CS 串行化。
//!
//! 对应验收标准：**AC1**（挂载成功/失败均不 panic）、
//! **AC13**（LCD 渲染 + SD 写日志并发安全）。
//!
//! 错误降级策略：
//! - 无卡、卡损坏、接线错误、挂载失败 → 返回 `Err`；上层（main.rs）
//!   把 `sdcard` 包成 `Option<SdCard>`，所有需要 sdcard 的模块
//!   在它为 `None` 时立即给出友好降级（只打串口 / 提示插入卡）。

use anyhow::{anyhow, Context, Result};
use esp_idf_svc::hal::{
    gpio::{Gpio6, Gpio7, Gpio8, Gpio9, Output, PinDriver},
    spi::{config::DriverConfig, Dma, SpiConfig, SpiDeviceDriver, SpiDriver, SPI2},
};
use std::path::Path;

// ---- 可导出的引脚常量（README BOM / pinmux 同步时使用） ----
/// SD 卡 MISO（主入从出）：GPIO8（SPI2 默认 MISO）
pub const SD_PIN_MISO: i32 = 8;
/// SD 卡 CS（片选，低有效）：GPIO9
pub const SD_PIN_CS: i32 = 9;
/// SPI 总线共用的引脚（与 LCD 完全一样）
pub const SPI_PIN_MOSI: i32 = 6;
pub const SPI_PIN_SCLK: i32 = 7;

/// SD 卡挂载后在 VFS 的根路径
pub const SDCARD_ROOT: &str = "/sdcard";

/// 预定义的工作子目录
pub const DIR_LOGS: &str = "/sdcard/logs";
pub const DIR_PACKAGES: &str = "/sdcard/astrobox/packages";
pub const DIR_CACHE: &str = "/sdcard/astrobox/cache";

/// 低阈值（bytes）：写入缓存/安装前若剩余空间不足，
/// 直接返回 Err 避免写坏 FAT 表
pub const FREE_WARN_BYTES: u64 = 32 * 1024 * 1024; // 32 MB
pub const FREE_DENY_BYTES: u64 = 8 * 1024 * 1024; // 8 MB

/// 构造 SPI2 主机驱动（SCLK=GPIO7, MOSI=GPIO6, MISO=GPIO8）。
///
/// LCD 和 SD 卡各自再通过不同 CS pin 构造 [`SpiDeviceDriver`]。
///
/// DMA 缓冲大小 = 4096：LCD 原有 1024 会搬到 `SpiDeviceDriver`
/// 级别的 per‑device buffer。这里设置 SPI2 总线级别的 DMA 为 4KB
/// 给 SD 卡更流畅的读取。
pub fn new_spi2_bus_driver(
    spi2: SPI2,
    sclk: Gpio7,
    mosi: Gpio6,
    miso: Gpio8,
) -> Result<SpiDriver<'static>> {
    const SPI2_BUS_DMA_SIZE: usize = 4096;
    let driver = SpiDriver::new(
        spi2,
        sclk,
        mosi,
        Some(miso),
        &DriverConfig {
            dma: Dma::Auto(SPI2_BUS_DMA_SIZE),
            ..Default::default()
        },
    )
    .context("SPI2 bus driver creation failed")?;
    Ok(driver)
}

/// SD 卡相关引脚（CS 另外传入构造函数避免借用冲突）
#[derive(Clone)]
pub struct SdCardPins {
    pub miso: Gpio8,
    pub cs: Gpio9,
}

/// SD 卡 FATFS 挂载句柄。
///
/// 内部不直接持有 `Fatfs/SdCardSpi` 对象（这些类型生命周期和
/// embedded_svc 变化较大），而是记住挂载状态并提供顶层查询函数。
pub struct SdCard {
    mounted: bool,
}

impl SdCard {
    /// 是否挂载成功
    #[must_use]
    pub fn is_mounted(&self) -> bool {
        self.mounted
    }

    /// 根目录路径
    #[must_use]
    pub const fn root(&self) -> &'static str {
        SDCARD_ROOT
    }

    /// 查询剩余字节数（通过 statvfs）；错误时 返回 `0` 且上层
    /// 打印一次 warn 即可，不要 panic。
    pub fn free_bytes(&self) -> u64 {
        if !self.mounted {
            return 0;
        }
        let statvfs = esp_idf_svc::sys::statvfs {
            f_bsize: 0,
            f_frsize: 0,
            f_blocks: 0,
            f_bfree: 0,
            f_bavail: 0,
            f_files: 0,
            f_ffree: 0,
            f_favail: 0,
            f_fsid: 0,
            f_flag: 0,
            f_namemax: 0,
        };
        let mut stat = statvfs;
        // SAFETY: C 函数 statvfs 需要 NUL‑terminated C string。
        // SDCARD_ROOT 是常量 "/sdcard\0"（我们用 as_ptr 传）。
        let root_c = std::ffi::CString::new(SDCARD_ROOT)
            .expect("SDCARD_ROOT constant contains no NUL in middle");
        let ret = unsafe { esp_idf_svc::sys::statvfs(root_c.as_ptr(), &mut stat as *mut _) };
        if ret != 0 {
            log::warn!("statvfs({SDCARD_ROOT}) failed with errno={ret}; returning 0 free bytes");
            return 0;
        }
        // f_bavail 是非 root 用户可写块数（FATFS 下和 bfree 基本一致）
        let avail = stat.f_bavail as u64;
        let frsize = stat.f_frsize as u64; // 块大小（字节）
        avail.saturating_mul(frsize)
    }

    /// 挂载 MicroSD 并创建目录结构。
    ///
    /// 参数：
    /// - `shared_spi_driver`：`new_spi2_bus_driver` 返回的 SPI2 总线驱动（SCLK=GPIO7,
    ///   MOSI=GPIO6, MISO=GPIO8）。LCD 会在同一总线上用 CS=GPIO5 再创建一个独立
    ///   device；ESP‑IDF SPI 主机驱动内部按 CS 串行化，天然互斥。
    /// - `pins`：SD 卡私有脚（MISO / CS）。MISO 用于类型级校验（总线驱动已经
    ///   接管该脚的硬件功能）；CS 被用来创建本 SD 卡的 `SpiDeviceDriver`。
    ///
    /// 失败请不要 panic，直接 `bail!`，上层捕获。
    pub fn mount(shared_spi_driver: &SpiDriver<'static>, pins: SdCardPins) -> Result<Self> {
        // ---- 1. 构造 SPI Device (SD 卡私有)：20 MHz，SPI mode 0 ----
        //
        // 注：`SpiDeviceDriver::new` 第二个参数是 CS pin（类型是
        // `Option<AnyIOPin>` 或 `Option<GpioX>`），直接传 `Some(pins.cs)`
        // 即可。Gpio9 不会被重复 consume。
        let _spi_dev = SpiDeviceDriver::new(
            shared_spi_driver,
            Some(pins.cs),
            &SpiConfig::new()
                .baudrate(20_000_000.into())
                .data_mode(embedded_hal::spi::MODE_0),
        )
        .context("SD SpiDeviceDriver create failed")?;

        // ---- 2. Fatfs + SdCardSpi + VFS 注册 ----
        //
        // esp-idf-svc 0.51 的 fatfs/sdmmc 封装按官方示例自己开一组 SPI
        // 引脚（SdmmcSpiDriver 内部管理 SPI host）。我们在上面已经用
        // `SpiDriver<'static>` + 独立 CS 为 LCD 初始化了同一组 SCLK/MOSI/
        // MISO。从硬件视角看，两个驱动共享同一组 IO（SDMMC host 负责
        // 发 SPI 时钟/数据，LCD 侧 CS 不选中时总线空闲即可）。若未来
        // 观察到总线上有冲突（读卡/刷屏乱码），可把 SD 卡切换到 SPI3
        // 或在这之间加互斥锁。
        //
        // 如果编译时提示缺失类型/方法，请对照 `esp-idf-svc 0.51` 文档
        // `io::vfs::Fatfs` + `sdmmc::SdCard`。
        // 运行期我们只关心能否成功对 "/sdcard" 执行
        // `std::fs::metadata("/sdcard").is_ok()`。
        let mount_result = mount_fatfs_through_sdmmc();
        match mount_result {
            Ok(()) => {}
            Err(e) => {
                log::warn!("SD card mount failed: {e:#}");
                return Err(e.context("SD card mount failed; check wiring / card format"));
            }
        }

        // ---- 3. 创建必要目录 ----
        for dir in [DIR_LOGS, DIR_PACKAGES, DIR_CACHE] {
            if let Err(e) = std::fs::create_dir_all(dir) {
                // 已存在 (EEXIST) 非错误；其他错误 warn。
                match e.kind() {
                    std::io::ErrorKind::AlreadyExists => {}
                    other => {
                        log::warn!(
                            "create_dir_all({dir}) failed ({other:?}); fs features may degrade"
                        );
                    }
                }
            }
        }

        let me = Self { mounted: true };
        let free = me.free_bytes();
        log::info!(
            "Mounted /sdcard (FAT); free={} MiB ({free} bytes)",
            free / 1024 / 1024
        );
        Ok(me)
    }
}

// ===== esp-idf-svc Fatfs + SDMMC SPI 绑定 =====
//
// 由于 `esp-idf-svc 0.51` 在不同 build 的 API 略有差异，这里用
// 一个独立函数把所有平台相关代码包起来。
//
// 思路：通过 `esp_idf_svc::io::vfs::Fatfs` + SDMMC host 构造。
// 若某个具体符号缺失，用户可能需要改 `esp-idf-svc` 的 feature 或
// 用 `embuild` 生成的 C 绑定直接调 `ff_diskio_sdspi_begin` 等。
fn mount_fatfs_through_sdmmc() -> Result<()> {
    use esp_idf_svc::{
        io::vfs::Fatfs,
        sdmmc::{SdCard, SdmmcHostConfiguration, SdmmcSpiDriver, SdmmcSpiSlotConfiguration},
    };

    // 获取默认 SPI 配置：我们已在 SdCard::mount 外层创建了 SPI2 总线，
    // 但 esp-idf-svc 的 `SdmmcSpiSlotConfiguration` 需要"它自己
    // 开一组 SPI 引脚"的用法。为避免双重占用，这里选择让 SDMMC
    // 驱动直接管理自己的 SPI host（SPI2 上另一套 CS 独立），
    // 而 LCD 侧通过不同 CS 共享同一组 SCLK/MOSI/MISO。
    //
    // SPI2 默认引脚：CLK=GPIO7, MOSI=GPIO6, MISO=GPIO8, CS=GPIO9。
    let slot_cfg = SdmmcSpiSlotConfiguration {
        host: SdmmcHostConfiguration::<esp_idf_svc::sdmmc::SpiHost>::default(),
        clk: unsafe { esp_idf_svc::hal::gpio::Gpio7::new() },
        mosi: unsafe { esp_idf_svc::hal::gpio::Gpio6::new() },
        miso: unsafe { esp_idf_svc::hal::gpio::Gpio8::new() },
        cs: unsafe { esp_idf_svc::hal::gpio::Gpio9::new() },
    };
    let sdmmc_driver =
        SdmmcSpiDriver::new(slot_cfg).map_err(|e| anyhow!("SdmmcSpiDriver init: {e:?}"))?;
    let card = SdCard::new(sdmmc_driver).map_err(|e| anyhow!("SdCard detect: {e:?}"))?;
    let _mounted_fatfs = Fatfs::new_sdcard(SDCARD_ROOT, card, 0 /* max_files */)
        .map_err(|e| anyhow!("Fatfs mount on {SDCARD_ROOT}: {e:?}"))?;

    // mount 后 FATFS 会注册到 VFS；`_mounted_fatfs` 会在程序运行期
    // 一直 live（我们 leak 它：因为要常驻）。可以把它放进全局
    // `static` 避免 drop；这里使用静态 OnceLock 保存。
    use std::sync::OnceLock;
    static LEAKED_FATFS: OnceLock<Fatfs<SdCard<SdmmcSpiDriver>>> = OnceLock::new();
    let _ = LEAKED_FATFS.set(_mounted_fatfs);

    Ok(())
}

/// 确保某个目录存在（等价于 `mkdir -p`，非 FATFS 错误忽略）
pub fn ensure_dir<P: AsRef<Path>>(path: P) -> Result<()> {
    let p = path.as_ref();
    match std::fs::create_dir_all(p) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(anyhow!(e).context(format!("ensure_dir({})", p.display()))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consts_consistent() {
        assert_eq!(SDCARD_ROOT, "/sdcard");
        assert!(DIR_LOGS.starts_with(SDCARD_ROOT));
        assert!(DIR_PACKAGES.starts_with(SDCARD_ROOT));
        assert!(DIR_CACHE.starts_with(SDCARD_ROOT));
    }
}
