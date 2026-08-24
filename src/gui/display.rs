//! LCD (ST7789, 240×320) 显示初始化。
//!
//! 注意：为了让 LCD 和 SD 卡能在同一根 SPI2 总线上通过不同 CS
//! 共存，本文件不再内部创建 `SpiDriver`，而是接受上层（main.rs）
//! 已经构造好的 `&SpiDriver<'static>`，再基于 LCD CS (GPIO5)
//! 创建 `SpiDeviceDriver`。
//!
//! 对应验收标准：**AC13**（SPI 互斥并发安全）。

use anyhow::{anyhow, Result};
use esp_idf_svc::hal::{
    delay::Delay,
    gpio::{Gpio2, Gpio3, Gpio4, Gpio5, PinDriver},
    ledc::{config::TimerConfig, LedcDriver, LedcTimerDriver, LEDC},
    spi::{SpiConfig, SpiDeviceDriver, SpiDriver},
};
use mipidsi::{
    interface::SpiInterface,
    models::ST7789,
    options::{ColorInversion, ColorOrder, Orientation, RefreshOrder},
    Builder,
};

type DisplayDcPin<'d> = PinDriver<'d, Gpio4, esp_idf_svc::hal::gpio::Output>;
type DisplayRstPin<'d> = PinDriver<'d, Gpio3, esp_idf_svc::hal::gpio::Output>;
type DisplayInterface<'d> = SpiInterface<'d, SpiDeviceDriver<'d, SpiDriver<'d>>, DisplayDcPin<'d>>;
pub type DisplayType<'d> = mipidsi::Display<DisplayInterface<'d>, ST7789, DisplayRstPin<'d>>;

const DISPLAY_SPI_BUFFER_SIZE: usize = 1024;
// SAFETY: This buffer is accessed only from the display initialization function
// which runs in a single-threaded context (no race conditions). The buffer is
// 'static (lives for the entire program) and is used exclusively as a DMA
// transfer buffer for the SPI interface. The mutable reference is created
// once during init and never shared or aliased.
static mut DISPLAY_SPI_BUFFER: [u8; DISPLAY_SPI_BUFFER_SIZE] = [0; DISPLAY_SPI_BUFFER_SIZE];

pub struct DisplayPins {
    pub backlight: Gpio2,
    pub rst: Gpio3,
    pub dc: Gpio4,
    pub cs: Gpio5,
}

/// 构造 LCD SPI 设备（40 MHz）并初始化 ST7789 芯片。
///
/// `shared_spi_bus` 来自 `sdcard::new_spi2_bus_driver`（SCLK=GPIO7,
/// MOSI=GPIO6, MISO=GPIO8）。本函数只用 LCD CS=GPIO5 创建 device。
pub fn init_display_st7789(
    shared_spi_bus: &SpiDriver<'static>,
    ledc: LEDC,
    pins: DisplayPins,
) -> Result<(DisplayType<'static>, LedcDriver<'static>)> {
    let DisplayPins {
        backlight,
        rst,
        dc,
        cs,
    } = pins;
    let LEDC {
        timer0, channel0, ..
    } = ledc;

    let dc = PinDriver::output(dc)?; // D/C
    let rst = PinDriver::output(rst)?; // RST

    let ledc_timer = LedcTimerDriver::new(timer0, &TimerConfig::new().frequency(25_000.into()))?;
    let mut backlight = LedcDriver::new(channel0, ledc_timer, backlight)?;
    backlight.set_duty(backlight.get_max_duty() / 2)?;

    // LCD 设备：40 MHz。SD 设备是 20 MHz，CS 分开所以互不影响。
    let spi_dev = SpiDeviceDriver::new(
        shared_spi_bus,
        Some(cs), // CS (GPIO5) — 和 SD 卡 GPIO9 独立
        &SpiConfig::new().baudrate(40_000_000.into()),
    )?;

    // SAFETY: DISPLAY_SPI_BUFFER is a static mut buffer that is only accessed
    // from this single initialization path (single-threaded). The buffer is
    // valid for 'static lifetime and will not be concurrently accessed.
    #[allow(static_mut_refs)]
    let buffer: &'static mut [u8] = unsafe { &mut DISPLAY_SPI_BUFFER };
    let di = SpiInterface::new(spi_dev, dc, buffer);

    let mut delay = Delay::new_default();
    let display = Builder::new(ST7789, di)
        .reset_pin(rst)
        .invert_colors(ColorInversion::Normal)
        .color_order(ColorOrder::Bgr)
        .orientation(Orientation::new().rotate(mipidsi::options::Rotation::Deg0))
        .refresh_order(RefreshOrder::new(
            mipidsi::options::VerticalRefreshOrder::TopToBottom,
            mipidsi::options::HorizontalRefreshOrder::LeftToRight,
        ))
        .display_size(240, 320)
        .display_offset(0, 0)
        .init(&mut delay)
        .map_err(|e| anyhow!("display init failed: {:?}", e))?;

    Ok((display, backlight))
}
