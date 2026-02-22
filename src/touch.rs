use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use cst816s::{TouchEvent as CstTouchEvent, CST816S};
use esp_idf_svc::hal::{
    delay::Delay,
    gpio::{Gpio0, Gpio1, Gpio16, Gpio18, Input, Output, PinDriver, Pull},
    i2c::{config::Config as I2cConfig, I2cDriver, I2C0},
    units::Hertz,
};

use crate::gui::slint_ui::{self, PointerAction, DISPLAY_HEIGHT, DISPLAY_WIDTH};

const POLL_INTERVAL: Duration = Duration::from_millis(10);
const I2C_FREQUENCY: Hertz = Hertz(400_000);

type TouchController = CST816S<
    I2cDriver<'static>,
    PinDriver<'static, Gpio1, Input>,
    PinDriver<'static, Gpio0, Output>,
>;

pub struct TouchPins {
    pub sda: Gpio18,
    pub scl: Gpio16,
    pub interrupt: Gpio1,
    pub reset: Gpio0,
}

pub fn spawn_touch_task(i2c: I2C0, pins: TouchPins) -> Result<()> {
    let TouchPins {
        sda,
        scl,
        interrupt,
        reset,
    } = pins;

    let i2c = I2cDriver::new(i2c, sda, scl, &I2cConfig::new().baudrate(I2C_FREQUENCY))
        .context("failed to configure I2C bus for CST816S")?;

    let mut int_pin = PinDriver::input(interrupt)?;
    int_pin.set_pull(Pull::Up)?;
    let rst_pin = PinDriver::output(reset)?;

    let mut delay = Delay::new_default();
    let mut controller = CST816S::new(i2c, int_pin, rst_pin);
    controller
        .setup(&mut delay)
        .map_err(|err| anyhow!("touch controller setup failed: {:?}", err))?;

    tokio::task::spawn_local(async move {
        if let Err(err) = touch_loop(controller).await {
            log::error!("touch loop exited: {err:?}");
        }
    });

    Ok(())
}

async fn touch_loop(mut controller: TouchController) -> Result<()> {
    let mut pointer_active = false;
    loop {
        if let Some(event) = controller.read_one_touch_event(true) {
            pointer_active = handle_touch_event(event, pointer_active)?;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

fn handle_touch_event(event: CstTouchEvent, pointer_active: bool) -> Result<bool> {
    let (x, y) = normalize_coordinates(event.x, event.y);
    let action_desc = match event.action {
        0 => "down",
        1 => "up",
        2 => "move",
        _ => "unknown",
    };

    slint_ui::set_touch_text(format!(
        "Touch {action} ({x:.0}, {y:.0})",
        action = action_desc
    ));

    let mut still_active = pointer_active;
    match event.action {
        0 => {
            slint_ui::dispatch_pointer_action(PointerAction::Press, (x, y))?;
            still_active = true;
        }
        1 => {
            if still_active {
                slint_ui::dispatch_pointer_action(PointerAction::Release, (x, y))?;
            }
            still_active = false;
        }
        2 => {
            if still_active {
                slint_ui::dispatch_pointer_action(PointerAction::Move, (x, y))?;
            } else {
                slint_ui::dispatch_pointer_action(PointerAction::Press, (x, y))?;
                still_active = true;
            }
        }
        _ => {}
    }

    Ok(still_active)
}

fn normalize_coordinates(raw_x: i32, raw_y: i32) -> (f32, f32) {
    let x = raw_x.clamp(0, (DISPLAY_WIDTH.saturating_sub(1)) as i32) as f32;
    let y = raw_y.clamp(0, (DISPLAY_HEIGHT.saturating_sub(1)) as i32) as f32;
    (x, y)
}
