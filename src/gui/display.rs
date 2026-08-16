use anyhow::{anyhow, Result};
use esp_idf_svc::hal::{
    delay::Delay,
    gpio::{Gpio2, Gpio3, Gpio4, Gpio5, Gpio6, Gpio7, PinDriver},
    ledc::{config::TimerConfig, LedcDriver, LedcTimerDriver, LEDC},
    spi::{config::DriverConfig, Dma, SpiConfig, SpiDeviceDriver, SpiDriver, SPI2},
};
use mipidsi::{
    interface::SpiInterface,
    models::GC9A01,
    options::{ColorInversion, ColorOrder, Orientation, RefreshOrder},
    Builder,
};

type DisplayDcPin<'d> = PinDriver<'d, Gpio4, esp_idf_svc::hal::gpio::Output>;
type DisplayRstPin<'d> = PinDriver<'d, Gpio3, esp_idf_svc::hal::gpio::Output>;
type DisplayInterface<'d> = SpiInterface<'d, SpiDeviceDriver<'d, SpiDriver<'d>>, DisplayDcPin<'d>>;
pub type DisplayType<'d> = mipidsi::Display<DisplayInterface<'d>, GC9A01, DisplayRstPin<'d>>;

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
    pub mosi: Gpio6,
    pub sclk: Gpio7,
}

pub fn init_display_gc9a01(
    spi2: SPI2,
    ledc: LEDC,
    pins: DisplayPins,
) -> Result<(DisplayType<'static>, LedcDriver<'static>)> {
    let DisplayPins {
        backlight,
        rst,
        dc,
        cs,
        mosi,
        sclk,
    } = pins;
    let LEDC {
        timer0, channel0, ..
    } = ledc;

    let dc = PinDriver::output(dc)?; // D/C
    let rst = PinDriver::output(rst)?; // RST

    let ledc_timer = LedcTimerDriver::new(timer0, &TimerConfig::new().frequency(25_000.into()))?;
    let mut backlight = LedcDriver::new(channel0, ledc_timer, backlight)?;
    backlight.set_duty(backlight.get_max_duty() / 2)?;

    let spi_driver = SpiDriver::new(
        spi2,
        sclk, // SCLK
        mosi, // MOSI (SDO)
        Option::<esp_idf_svc::hal::gpio::Gpio8>::None,
        &DriverConfig {
            dma: Dma::Auto(DISPLAY_SPI_BUFFER_SIZE),
            ..Default::default()
        },
    )?;
    let spi_dev = SpiDeviceDriver::new(
        spi_driver,
        Some(cs), // CS
        &SpiConfig::new().baudrate(40_000_000.into()),
    )?;

    // SAFETY: DISPLAY_SPI_BUFFER is a static mut buffer that is only accessed
    // from this single initialization path (single-threaded). The buffer is
    // valid for 'static lifetime and will not be concurrently accessed.
    #[allow(static_mut_refs)]
    let buffer: &'static mut [u8] = unsafe { &mut DISPLAY_SPI_BUFFER };
    let di = SpiInterface::new(spi_dev, dc, buffer);

    let mut delay = Delay::new_default();
    let display = Builder::new(GC9A01, di)
        .reset_pin(rst)
        .invert_colors(ColorInversion::Inverted)
        .color_order(ColorOrder::Rgb)
        .orientation(Orientation::new().rotate(mipidsi::options::Rotation::Deg0))
        .refresh_order(RefreshOrder::new(
            mipidsi::options::VerticalRefreshOrder::TopToBottom,
            mipidsi::options::HorizontalRefreshOrder::LeftToRight,
        ))
        .display_size(240, 240)
        .display_offset(0, 0)
        .init(&mut delay)
        .map_err(|e| anyhow!("display init failed: {:?}", e))?;

    Ok((display, backlight))
}
