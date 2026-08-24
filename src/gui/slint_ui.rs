use std::{
    cell::RefCell,
    collections::VecDeque,
    mem,
    ops::Range,
    rc::Rc,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use anyhow::{anyhow, Result};
use embedded_graphics_core::{
    draw_target::DrawTarget,
    pixelcolor::{raw::RawU16, Rgb565},
    prelude::{Point, Size},
    primitives::Rectangle,
};
use esp_idf_svc::sys::esp_get_free_heap_size;
use slint::{
    platform::{
        self,
        software_renderer::{
            LineBufferProvider, MinimalSoftwareWindow, RepaintBufferType, Rgb565Pixel,
        },
        Platform, PointerEventButton, WindowAdapter,
    },
    LogicalPosition, PhysicalSize, SharedString,
};
use tokio::sync::mpsc;

use super::display::DisplayType;

slint::include_modules!();

// ===== 资源面板 UI → 主逻辑 的事件通道 =====
//
// 初始化一次：`take_resource_ui_event_rx()` 返回 rx（只返回一次，避免多消费者）。
// 回调函数（Slint 线程）调用 `send_resource_event(ResourceUiEvent::...)`。
// 事件携带的数据尽量轻量（索引 / tab 号），重数据由 main 侧查缓存。

/// 资源面板上发生的用户事件。`usize` / `i32` 便于跨线程传。
#[derive(Clone, Debug)]
pub enum ResourceUiEvent {
    /// 长按 ⚙ 按钮 → 要求打开 / 切换可见性
    SettingsLongPressed,
    /// 关闭按钮（面板顶部 ×）
    ClosePanel,
    /// Tab 切换：0=本地(SD), 1=AstroBox（原本的 2=米坛 因 BandBBS 合规已移除，不再发出）
    SourceSwitched(i32),
    /// 上一页
    PrevPage,
    /// 下一页
    NextPage,
    /// 点击列表某一行 (0..5)
    RowPressed(i32),
}

static RESOURCE_EVENT_TX: OnceLock<mpsc::Sender<ResourceUiEvent>> = OnceLock::new();
static RESOURCE_EVENT_RX_ONCE: OnceLock<Mutex<Option<mpsc::Receiver<ResourceUiEvent>>>> =
    OnceLock::new();

/// 创建资源事件 channel（容量 16，足够覆盖按键抖动）。返回的 receiver 只能被取一次；
/// 第二次调用返回 `None`（上层应 panic 或忽略）。
pub fn init_resource_ui_event_channel() -> Option<mpsc::Receiver<ResourceUiEvent>> {
    let (tx, rx) = mpsc::channel::<ResourceUiEvent>(16);
    let _ = RESOURCE_EVENT_TX.set(tx);
    let _ = RESOURCE_EVENT_RX_ONCE.set(Mutex::new(Some(rx)));
    take_resource_ui_event_rx()
}

/// 获取资源事件 Receiver（一次性）。
pub fn take_resource_ui_event_rx() -> Option<mpsc::Receiver<ResourceUiEvent>> {
    match RESOURCE_EVENT_RX_ONCE.get() {
        Some(g) => match g.lock() {
            Ok(mut slot) => slot.take(),
            Err(_poisoned) => None,
        },
        None => None,
    }
}

fn send_resource_event(event: ResourceUiEvent) {
    if let Some(tx) = RESOURCE_EVENT_TX.get() {
        // Slint 回调是同步的，用 try_send 避免阻塞 UI 线程。
        // 满时丢弃旧事件（主逻辑消费速度跟不上时保守处理）。
        match tx.try_send(event) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                log::debug!("ResourceUiEvent channel full; event dropped");
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                // receiver 端已经退出（罕见），忽略即可。
            }
        }
    }
}

pub const DISPLAY_WIDTH: usize = 240;
pub const DISPLAY_HEIGHT: usize = 320;
const MAX_PENDING_POINTER_EVENTS: usize = 64;
const STATS_UPDATE_INTERVAL: Duration = Duration::from_secs(1);
const ENABLE_DEBUG_STATS: bool = false;

thread_local! {
    static PLATFORM_WINDOW: RefCell<Option<Rc<MinimalSoftwareWindow>>> =
        const { RefCell::new(None) };
    static FRAME_STATS: RefCell<FrameStats> = RefCell::new(FrameStats::new());
    static APP_INSTANCE: RefCell<Option<App>> = const { RefCell::new(None) };
}

static UI_UPDATES: OnceLock<Mutex<PendingUiUpdates>> = OnceLock::new();

fn ui_updates() -> &'static Mutex<PendingUiUpdates> {
    UI_UPDATES.get_or_init(|| Mutex::new(PendingUiUpdates::default()))
}

#[derive(Default)]
struct PendingUiUpdates {
    touch_text: Option<String>,
    device_connected: Option<bool>,
    connected_device_count: Option<i32>,
    device_status: Option<DeviceStatusUi>,
    // ---- 资源面板 ----
    resource_panel_visible: Option<bool>,
    repo_source_tab: Option<i32>,
    list_items: Option<[String; 5]>,
    list_page: Option<i32>,
    list_total: Option<i32>,
    install_progress_text: Option<String>,
    pointer_events: VecDeque<QueuedPointerEvent>,
}

#[derive(Clone, Copy)]
struct QueuedPointerEvent {
    action: PointerAction,
    position: (f32, f32),
}

#[derive(Clone, Default)]
pub struct DeviceStatusUi {
    pub device_name: String,
    pub battery_percent: i32,
    pub charge_text: String,
    pub net_up_text: String,
    pub net_down_text: String,
}

pub fn render_hello_world(display: &mut DisplayType<'static>) -> Result<()> {
    let window = ensure_platform_window()?;

    ensure_app()?;
    apply_pending_ui_updates(&window);

    let frame_start = Instant::now();
    if ENABLE_DEBUG_STATS {
        if let Some(stats_text) = FRAME_STATS.with(|cell| {
            cell.borrow_mut()
                .build_stats_text_if_due(frame_start, unsafe { esp_get_free_heap_size() })
        }) {
            set_stats_text(stats_text);
        }
    }

    platform::update_timers_and_animations();

    let render_error = RefCell::<Option<anyhow::Error>>::new(None);
    let display_ptr: *mut DisplayType<'static> = display;
    let mut line_buffer = [Rgb565Pixel(0); DISPLAY_WIDTH];

    if window.draw_if_needed(|renderer| {
        if render_error.borrow().is_some() {
            return;
        }

        // SAFETY: The draw loop is single-threaded and guarantees no aliasing
        // with other uses of the display. The raw pointer is created from a
        // valid &mut reference passed to render_hello_world and only used within
        // this closure. The 'static lifetime is guaranteed by the function signature.
        let display_ref = unsafe { &mut *display_ptr };
        let mut provider = DisplayLineProvider::new(display_ref, &mut line_buffer, &render_error);
        renderer.render_by_line(&mut provider);
        if let Err(err) = provider.finish() {
            *render_error.borrow_mut() = Some(err);
        }
    }) {
        platform::update_timers_and_animations();
    }

    if let Some(err) = render_error.into_inner() {
        return Err(err);
    }

    let render_duration = frame_start.elapsed();
    FRAME_STATS.with(|cell| {
        cell.borrow_mut()
            .update_after_frame(frame_start, render_duration);
    });

    Ok(())
}

const MAX_BATCH_LINES: usize = 32;

struct DisplayLineProvider<'a, 'b> {
    display: &'a mut DisplayType<'static>,
    line_buffer: &'b mut [Rgb565Pixel; DISPLAY_WIDTH],
    accumulator: LineAccumulator,
    error: &'b RefCell<Option<anyhow::Error>>,
}

impl<'a, 'b> DisplayLineProvider<'a, 'b> {
    fn new(
        display: &'a mut DisplayType<'static>,
        line_buffer: &'b mut [Rgb565Pixel; DISPLAY_WIDTH],
        error: &'b RefCell<Option<anyhow::Error>>,
    ) -> Self {
        Self {
            display,
            line_buffer,
            accumulator: LineAccumulator::new(),
            error,
        }
    }

    fn finish(&mut self) -> Result<()> {
        self.accumulator.flush(self.display)
    }
}

impl<'a, 'b, 'c> LineBufferProvider for &'c mut DisplayLineProvider<'a, 'b> {
    type TargetPixel = Rgb565Pixel;

    fn process_line(
        &mut self,
        line: usize,
        range: Range<usize>,
        render_fn: impl FnOnce(&mut [Self::TargetPixel]),
    ) {
        if self.error.borrow().is_some() {
            return;
        }

        let segment = &mut self.line_buffer[range.clone()];
        render_fn(segment);

        if let Err(err) = self
            .accumulator
            .push_line(line, range, segment, self.display)
        {
            *self.error.borrow_mut() = Some(err);
        }
    }
}

struct LineAccumulator {
    start_line: usize,
    range: Range<usize>,
    line_count: usize,
    buffer: Vec<Rgb565Pixel>,
}

impl LineAccumulator {
    fn new() -> Self {
        Self {
            start_line: 0,
            range: 0..0,
            line_count: 0,
            buffer: Vec::with_capacity(DISPLAY_WIDTH * MAX_BATCH_LINES),
        }
    }

    fn push_line(
        &mut self,
        line: usize,
        range: Range<usize>,
        pixels: &[Rgb565Pixel],
        display: &mut DisplayType<'static>,
    ) -> Result<()> {
        if pixels.is_empty() {
            return Ok(());
        }

        if self.line_count == 0 {
            self.start_line = line;
            self.range = range.clone();
        } else {
            let expected_line = self.start_line + self.line_count;
            if line != expected_line
                || range.start != self.range.start
                || range.end != self.range.end
            {
                self.flush(display)?;
                self.start_line = line;
                self.range = range.clone();
            }
        }

        self.buffer.extend_from_slice(pixels);
        self.line_count += 1;

        if self.line_count >= MAX_BATCH_LINES {
            self.flush(display)?;
        }
        Ok(())
    }

    fn flush(&mut self, display: &mut DisplayType<'static>) -> Result<()> {
        if self.line_count == 0 {
            return Ok(());
        }

        let rect = Rectangle::new(
            Point::new(self.range.start as i32, self.start_line as i32),
            Size::new(self.range.len() as u32, self.line_count as u32),
        );

        let colors = self
            .buffer
            .iter()
            .take(self.range.len() * self.line_count)
            .map(|Rgb565Pixel(pixel)| Rgb565::from(RawU16::new(*pixel)));

        display
            .fill_contiguous(&rect, colors)
            .map_err(|e| anyhow!("Failed to refresh region {:?}: {e:?}", rect))?;

        self.buffer.clear();
        self.line_count = 0;
        Ok(())
    }
}

#[derive(Clone)]
struct SimplePlatform {
    window: Rc<MinimalSoftwareWindow>,
    start: Instant,
}

impl Platform for SimplePlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }

    fn run_event_loop(&self) -> Result<(), slint::PlatformError> {
        Ok(())
    }

    fn duration_since_start(&self) -> std::time::Duration {
        self.start.elapsed()
    }
}

fn ensure_platform_window() -> Result<Rc<MinimalSoftwareWindow>> {
    PLATFORM_WINDOW.with(|cell| {
        if let Some(existing) = cell.borrow().as_ref() {
            return Ok(existing.clone());
        }

        let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        window.set_size(PhysicalSize::new(DISPLAY_WIDTH as _, DISPLAY_HEIGHT as _));
        let platform = SimplePlatform {
            window: window.clone(),
            start: Instant::now(),
        };
        platform::set_platform(Box::new(platform))
            .map_err(|e| anyhow!("Failed to set Slint platform: {e:?}"))?;
        *cell.borrow_mut() = Some(window.clone());
        Ok(window)
    })
}

fn ensure_app() -> Result<()> {
    APP_INSTANCE.with(|cell| {
        if cell.borrow().is_none() {
            let app = App::new().map_err(|e| anyhow!("Failed to create Slint App: {:?}", e))?;

            // ===== 注册资源面板回调（发送事件到 main 侧事件循环消费） =====
            app.on_settings_long_pressed(|| {
                send_resource_event(ResourceUiEvent::SettingsLongPressed);
            });
            app.on_resource_close(|| {
                send_resource_event(ResourceUiEvent::ClosePanel);
            });
            app.on_source_switched(|tab| {
                send_resource_event(ResourceUiEvent::SourceSwitched(tab));
            });
            app.on_list_prev_page(|| {
                send_resource_event(ResourceUiEvent::PrevPage);
            });
            app.on_list_next_page(|| {
                send_resource_event(ResourceUiEvent::NextPage);
            });
            app.on_list_item_pressed(|row| {
                send_resource_event(ResourceUiEvent::RowPressed(row));
            });

            app.show()
                .map_err(|e| anyhow!("Failed to show Slint App: {:?}", e))?;
            cell.replace(Some(app));
        }
        Ok(())
    })
}

fn set_stats_text(stats: SharedString) {
    APP_INSTANCE.with(|cell| {
        if let Some(app) = cell.borrow().as_ref() {
            app.set_stats_text(stats.clone());
            PLATFORM_WINDOW.with(|window_cell| {
                if let Some(window) = window_cell.borrow().as_ref() {
                    window.request_redraw();
                }
            });
        }
    });
}

struct FrameStats {
    last_frame_start: Option<Instant>,
    last_render_time: Option<Duration>,
    last_fps: f32,
    last_stats_update: Option<Instant>,
    last_stats_text: SharedString,
}

impl FrameStats {
    fn new() -> Self {
        Self {
            last_frame_start: None,
            last_render_time: None,
            last_fps: 0.0,
            last_stats_update: None,
            last_stats_text: SharedString::from(""),
        }
    }

    fn snapshot_for_display(&self) -> (f32, Option<Duration>) {
        (self.last_fps, self.last_render_time)
    }

    fn update_after_frame(&mut self, frame_start: Instant, render_time: Duration) {
        if let Some(previous_start) = self.last_frame_start {
            if let Some(frame_interval) = frame_start.checked_duration_since(previous_start) {
                let frame_time = frame_interval.as_secs_f32();
                if frame_time > f32::EPSILON {
                    self.last_fps = 1.0 / frame_time;
                }
            }
        }
        self.last_frame_start = Some(frame_start);
        self.last_render_time = Some(render_time);
    }

    fn build_stats_text_if_due(&mut self, now: Instant, heap_bytes: u32) -> Option<SharedString> {
        if let Some(last) = self.last_stats_update {
            if now.saturating_duration_since(last) < STATS_UPDATE_INTERVAL {
                return None;
            }
        }

        let (displayed_fps, last_render_duration) = self.snapshot_for_display();
        let fps_display = if displayed_fps > f32::EPSILON {
            format!("{displayed_fps:.1}")
        } else {
            "--".to_string()
        };
        let render_display = if let Some(duration) = last_render_duration {
            format!("{:.2}", duration.as_secs_f32() * 1_000.0)
        } else {
            "--".to_string()
        };
        let heap_kb = heap_bytes as f32 / 1024.0;
        let next_text = SharedString::from(format!(
            "FPS: {fps}\nRender: {render} ms\nHeap: {heap:.1} KB",
            fps = fps_display,
            render = render_display,
            heap = heap_kb
        ));

        self.last_stats_update = Some(now);
        if next_text == self.last_stats_text {
            return None;
        }

        self.last_stats_text = next_text.clone();
        Some(next_text)
    }
}

#[derive(Clone, Copy, Debug)]
pub enum PointerAction {
    Press,
    Move,
    Release,
}

pub fn dispatch_pointer_action(action: PointerAction, position: (f32, f32)) -> Result<()> {
    let mut updates = match ui_updates().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };

    if updates.pointer_events.len() >= MAX_PENDING_POINTER_EVENTS {
        updates.pointer_events.pop_front();
    }
    updates
        .pointer_events
        .push_back(QueuedPointerEvent { action, position });
    Ok(())
}

pub fn set_touch_text(stats: String) {
    let mut updates = match ui_updates().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    updates.touch_text = Some(stats);
}

pub fn set_device_connected(connected: bool) {
    let mut updates = match ui_updates().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    updates.device_connected = Some(connected);
}

pub fn set_connected_device_count(count: usize) {
    let mut updates = match ui_updates().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    updates.connected_device_count = Some(count as i32);
}

pub fn set_device_status(status: DeviceStatusUi) {
    let mut updates = match ui_updates().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    updates.device_status = Some(status);
}

// ===== 资源面板 setters（由 main.rs / repo 监听器调用） =====
pub fn set_resource_panel_visible(visible: bool) {
    let mut updates = match ui_updates().lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    updates.resource_panel_visible = Some(visible);
}

pub fn set_repo_source_tab(tab: i32) {
    let mut updates = match ui_updates().lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    updates.repo_source_tab = Some(tab);
}

/// 设置列表 5 行显示文本；空串表示该行空。
pub fn set_list_items(items: [String; 5]) {
    let mut updates = match ui_updates().lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    updates.list_items = Some(items);
}

pub fn set_list_page(page: i32, total: i32) {
    let mut updates = match ui_updates().lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    updates.list_page = Some(page);
    updates.list_total = Some(total);
}

/// 设置安装 / 加载中文本（显示在面板进度条位置或主界面"安装进度文字"位置）。
/// 空串表示清空。
pub fn set_install_progress_text(text: String) {
    let mut updates = match ui_updates().lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    updates.install_progress_text = Some(text);
}

fn apply_pending_ui_updates(window: &Rc<MinimalSoftwareWindow>) {
    let (
        touch_text,
        device_connected,
        connected_device_count,
        device_status,
        pointer_events,
        panel_visible,
        tab,
        list_items,
        page_and_total,
        progress_text,
    ) = {
        let mut updates = match ui_updates().lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        (
            updates.touch_text.take(),
            updates.device_connected.take(),
            updates.connected_device_count.take(),
            updates.device_status.take(),
            mem::take(&mut updates.pointer_events),
            updates.resource_panel_visible.take(),
            updates.repo_source_tab.take(),
            updates.list_items.take(),
            match (updates.list_page.take(), updates.list_total.take()) {
                (Some(p), Some(t)) => Some((p, t)),
                (Some(p), None) => Some((p, -1)),
                (None, Some(t)) => Some((-1, t)),
                (None, None) => None,
            },
            updates.install_progress_text.take(),
        )
    };

    if touch_text.is_none()
        && device_connected.is_none()
        && connected_device_count.is_none()
        && device_status.is_none()
        && pointer_events.is_empty()
        && panel_visible.is_none()
        && tab.is_none()
        && list_items.is_none()
        && page_and_total.is_none()
        && progress_text.is_none()
    {
        return;
    }

    APP_INSTANCE.with(|cell| {
        if let Some(app) = cell.borrow().as_ref() {
            if let Some(text) = touch_text {
                app.set_touch_text(SharedString::from(text));
            }
            if let Some(connected) = device_connected {
                app.set_connected(connected);
            }
            if let Some(count) = connected_device_count {
                app.set_connected_device_count(count);
            }
            if let Some(status) = device_status {
                app.set_device_name(SharedString::from(status.device_name));
                app.set_battery_percent(status.battery_percent);
                app.set_charge_text(SharedString::from(status.charge_text));
                app.set_net_up_text(SharedString::from(status.net_up_text));
                app.set_net_down_text(SharedString::from(status.net_down_text));
            }
            if let Some(vis) = panel_visible {
                app.set_resource_panel(vis);
            }
            if let Some(t) = tab {
                app.set_repo_source_tab(t);
            }
            if let Some(items) = list_items {
                app.set_list_item_0(SharedString::from(items[0].clone()));
                app.set_list_item_1(SharedString::from(items[1].clone()));
                app.set_list_item_2(SharedString::from(items[2].clone()));
                app.set_list_item_3(SharedString::from(items[3].clone()));
                app.set_list_item_4(SharedString::from(items[4].clone()));
            }
            if let Some((p, t)) = page_and_total {
                if p >= 0 {
                    app.set_list_page(p);
                }
                if t >= 0 {
                    app.set_list_total(t);
                }
            }
            if let Some(ptext) = progress_text {
                app.set_install_progress_text(SharedString::from(ptext));
            }
        }
    });

    for event in pointer_events {
        window.dispatch_event(pointer_to_window_event(event.action, event.position));
    }
    window.request_redraw();
}

fn pointer_to_window_event(
    action: PointerAction,
    position: (f32, f32),
) -> slint::platform::WindowEvent {
    let logical_position = LogicalPosition::new(position.0, position.1);
    match action {
        PointerAction::Press => slint::platform::WindowEvent::PointerPressed {
            position: logical_position,
            button: PointerEventButton::Left,
        },
        PointerAction::Move => slint::platform::WindowEvent::PointerMoved {
            position: logical_position,
        },
        PointerAction::Release => slint::platform::WindowEvent::PointerReleased {
            position: logical_position,
            button: PointerEventButton::Left,
        },
    }
}
