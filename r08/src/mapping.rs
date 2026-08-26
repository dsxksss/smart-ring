//! Platform-independent mapping from R08 HID/GATT input to host actions.
//!
//! Confirmed hardware limits:
//! - Custom GATT `0x1D` is a discrete action code, not a touch stream.
//! - HID reports relative Y, not Wheel; this engine converts Y to wheel.
//! - Actions 4/5 and camera/long-press have no stable mapping.

use std::collections::VecDeque;

use crate::protocol::{
    checksum_ok, describe_colmi_packet, format_packet, R08_TAP_DEBOUNCE_MS, R08_TAP_FLUSH_MS,
    WHEEL_DELTA,
};

pub const LEFT_BUTTON_DOWN: u16 = 0x0001;
pub const LEFT_BUTTON_UP: u16 = 0x0002;
pub const VERTICAL_WHEEL: u16 = 0x0400;
pub const HORIZONTAL_WHEEL: u16 = 0x0800;
const GATT_SWIPE_SUPPRESS_MS: u64 = 500;
const HID_AXIS_MAX: i32 = 32;
const SMOOTH_STEP: i32 = 16;
const MAX_QUEUED_STEPS: usize = 96;
const GATT_SWIPE_DISTANCE: i32 = 240;
const HOLD_START_MS: u64 = 300;
const HOLD_MID_MS: u64 = 1500;
const HOLD_FAST_MS: u64 = 3000;
const HOLD_SLOW_STEP: i32 = 2;
const HOLD_MID_STEP: i32 = 4;
const HOLD_FAST_STEP: i32 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HidMouseEvent {
    pub is_ring: bool,
    pub button_flags: u16,
    pub button_data: i16,
    pub dx: i32,
    pub dy: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputEvent {
    GattPacket(Vec<u8>),
    HidMouse(HidMouseEvent),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Output {
    Wheel(i32),
    CaptureCursorAnchor,
    RestoreCursor,
    ReleaseLeftButton,
    Copy,
    Paste,
    Log(String),
}

#[derive(Debug, Clone, Copy)]
pub struct MappingConfig {
    pub scroll_gain: i32,
    pub inject: bool,
}

impl Default for MappingConfig {
    fn default() -> Self {
        Self {
            scroll_gain: 4,
            inject: false,
        }
    }
}

pub struct MappingEngine {
    config: MappingConfig,
    tap_count: u32,
    last_tap_ms: Option<u64>,
    tap_deadline_ms: Option<u64>,
    scroll_queue: VecDeque<i32>,
    scroll_direction: i32,
    held_direction: i32,
    held_started_ms: u64,
    held_output_magnitude: i32,
    ring_button_down: bool,
    ring_gesture_moved: bool,
    last_hid_vertical_ms: Option<u64>,
}

impl MappingEngine {
    pub fn new(config: MappingConfig) -> Self {
        Self {
            config,
            tap_count: 0,
            last_tap_ms: None,
            tap_deadline_ms: None,
            scroll_queue: VecDeque::new(),
            scroll_direction: 0,
            held_direction: 0,
            held_started_ms: 0,
            held_output_magnitude: 0,
            ring_button_down: false,
            ring_gesture_moved: false,
            last_hid_vertical_ms: None,
        }
    }

    pub fn inject_enabled(&self) -> bool {
        self.config.inject
    }

    pub fn set_inject_enabled(&mut self, enabled: bool) {
        self.config.inject = enabled;
        self.clear_pending_actions();
    }

    pub fn clear_pending_actions(&mut self) {
        self.tap_count = 0;
        self.last_tap_ms = None;
        self.tap_deadline_ms = None;
        self.scroll_queue.clear();
        self.scroll_direction = 0;
        self.reset_held_scroll();
        self.ring_button_down = false;
        self.ring_gesture_moved = false;
        self.last_hid_vertical_ms = None;
    }

    pub fn handle(&mut self, event: InputEvent, now_ms: u64) -> Vec<Output> {
        let mut out = Vec::new();
        match event {
            InputEvent::GattPacket(data) => self.handle_gatt(&data, now_ms, &mut out),
            InputEvent::HidMouse(mouse) => self.handle_hid(mouse, now_ms, &mut out),
        }
        self.flush_due_taps(now_ms, &mut out);
        self.filter_inject(out)
    }

    pub fn tick(&mut self, now_ms: u64) -> Vec<Output> {
        let mut out = Vec::new();
        self.flush_due_taps(now_ms, &mut out);
        if let Some(delta) = self.next_scroll_delta(now_ms) {
            if delta != 0 {
                out.push(Output::Wheel(delta));
            }
        }
        self.filter_inject(out)
    }

    fn filter_inject(&self, out: Vec<Output>) -> Vec<Output> {
        if self.config.inject {
            return out;
        }
        out.into_iter()
            .filter(|item| matches!(item, Output::Log(_)))
            .collect()
    }

    fn handle_gatt(&mut self, data: &[u8], now_ms: u64, out: &mut Vec<Output>) {
        let description = describe_colmi_packet(data);
        out.push(Output::Log(format!(
            "RX {}{}",
            format_packet(data),
            if description.is_empty() {
                String::new()
            } else {
                format!("  {description}")
            }
        )));
        if data.len() >= 2 && data[0] == 0x02 && data[1] == 0x02 {
            out.push(Output::Log(
                "ACTION R08 兼容按键通知（HID 已处理，不重复计数）".to_string(),
            ));
            return;
        }
        if data.len() < 2 || data[0] != 0x1D {
            return;
        }
        if data.len() == 16 && !checksum_ok(data) {
            return;
        }
        match data[1] {
            1 => {
                self.record_tap(now_ms, out);
                out.push(Output::Log("ACTION 点击（等待判断双击/三击）".to_string()));
            }
            2 => self.gatt_scroll(-1, now_ms, out),
            3 => self.gatt_scroll(1, now_ms, out),
            4 | 5 => out.push(Output::Log(format!(
                "ACTION 动作 {} 无稳定独立码，默认无操作",
                data[1]
            ))),
            other => out.push(Output::Log(format!("ACTION 未映射的触控动作 {other}"))),
        }
    }

    fn gatt_scroll(&mut self, direction: i32, now_ms: u64, out: &mut Vec<Output>) {
        if self
            .last_hid_vertical_ms
            .is_some_and(|previous| now_ms.saturating_sub(previous) < GATT_SWIPE_SUPPRESS_MS)
        {
            out.push(Output::Log(
                "ACTION GATT 上下滑已由 HID 相对 Y 处理，忽略离散动作以免重复滚动".to_string(),
            ));
            return;
        }
        self.queue_smooth_wheel(direction * GATT_SWIPE_DISTANCE);
        out.push(Output::Log(if direction > 0 {
            "ACTION 上滑 -> 向上平滑滚动".to_string()
        } else {
            "ACTION 下滑 -> 向下平滑滚动".to_string()
        }));
    }

    fn handle_hid(&mut self, input: HidMouseEvent, now_ms: u64, out: &mut Vec<Output>) {
        if !input.is_ring {
            if input.dx != 0 || input.dy != 0 {
                out.push(Output::CaptureCursorAnchor);
            }
            return;
        }
        out.push(Output::Log(format!(
            "HID_MOUSE_R08 buttons=0x{:04X} data={} dx={} dy={}",
            input.button_flags, input.button_data, input.dx, input.dy
        )));
        if input.button_flags & LEFT_BUTTON_DOWN != 0 {
            self.ring_button_down = true;
            self.ring_gesture_moved = false;
            self.reset_held_scroll();
            out.push(Output::ReleaseLeftButton);
            out.push(Output::Log(
                "ACTION R08 触控开始；已立即释放系统左键，避免拖拽/按住".to_string(),
            ));
        }
        if input.dx != 0 || input.dy != 0 {
            out.push(Output::RestoreCursor);
            let abs_x = input.dx.abs();
            let abs_y = input.dy.abs();
            if self.ring_button_down && input.dx == 0 && (1..=HID_AXIS_MAX).contains(&abs_y) {
                self.ring_gesture_moved = true;
                self.last_hid_vertical_ms = Some(now_ms);
                out.push(Output::ReleaseLeftButton);
                let direction = -input.dy.signum();
                self.start_held_scroll(direction, now_ms);
                out.push(Output::Log(format!(
                    "ACTION R08 滑动 dy={} -> 精细滚动方向已识别；短划一格，保持触摸连续滚动",
                    input.dy
                )));
            } else if self.ring_button_down && input.dy == 0 && (1..=HID_AXIS_MAX).contains(&abs_x)
            {
                self.ring_gesture_moved = true;
                self.reset_held_scroll();
                out.push(Output::Log(format!(
                    "ACTION R08 横滑 dx={} 无稳定独立动作码，已忽略",
                    input.dx
                )));
            } else {
                out.push(Output::Log(
                    "ACTION R08 前导/结束校准位移，已忽略".to_string(),
                ));
            }
        }
        if input.button_flags & LEFT_BUTTON_UP != 0 {
            let (held_direction, held_output) = self.finish_held_scroll();
            if self.ring_button_down && !self.ring_gesture_moved {
                self.record_tap(now_ms, out);
                out.push(Output::Log(
                    "ACTION R08 点击完成（等待判断双击/三击）".to_string(),
                ));
            } else if self.ring_button_down {
                if held_direction != 0 && held_output < WHEEL_DELTA {
                    self.queue_smooth_wheel(held_direction * (WHEEL_DELTA - held_output));
                    out.push(Output::Log(
                        "ACTION R08 短划完成 -> 滚动一个标准刻度".to_string(),
                    ));
                } else {
                    out.push(Output::Log(
                        "ACTION R08 持续滚动结束 -> 已立即停止，不计入点击次数".to_string(),
                    ));
                }
            }
            self.ring_button_down = false;
            self.ring_gesture_moved = false;
        }
        if input.button_flags & VERTICAL_WHEEL != 0 {
            out.push(Output::Log(format!(
                "ACTION HID 垂直滚轮 {}（由系统原生处理，程序不重复注入）",
                if input.button_data > 0 {
                    "向上"
                } else {
                    "向下"
                }
            )));
        }
        if input.button_flags & HORIZONTAL_WHEEL != 0 {
            out.push(Output::Log(
                "ACTION HID 水平滚轮无稳定映射，已忽略".to_string(),
            ));
        }
    }

    fn record_tap(&mut self, now_ms: u64, _out: &mut Vec<Output>) {
        if self
            .last_tap_ms
            .is_some_and(|previous| now_ms.saturating_sub(previous) < R08_TAP_DEBOUNCE_MS)
        {
            return;
        }
        self.last_tap_ms = Some(now_ms);
        self.tap_count += 1;
        self.tap_deadline_ms = Some(now_ms.saturating_add(R08_TAP_FLUSH_MS));
    }

    fn flush_due_taps(&mut self, now_ms: u64, out: &mut Vec<Output>) {
        let Some(deadline) = self.tap_deadline_ms else {
            return;
        };
        if now_ms < deadline {
            return;
        }
        let count = self.tap_count;
        self.tap_count = 0;
        self.tap_deadline_ms = None;
        match count {
            2 => {
                out.push(Output::Copy);
                out.push(Output::Log("ACTION 双击 -> 复制".to_string()));
            }
            n if n >= 3 => {
                out.push(Output::Paste);
                out.push(Output::Log("ACTION 三击 -> 粘贴".to_string()));
            }
            1 => out.push(Output::Log("ACTION 单击 -> 无操作".to_string())),
            _ => {}
        }
    }

    fn queue_smooth_wheel(&mut self, total_delta: i32) {
        let direction = total_delta.signum();
        if direction == 0 {
            return;
        }
        if self.scroll_direction != 0 && self.scroll_direction != direction {
            self.scroll_queue.clear();
        }
        self.scroll_direction = direction;
        let mut remaining = total_delta.abs();
        while remaining > 0 && self.scroll_queue.len() < MAX_QUEUED_STEPS {
            let step = remaining.min(SMOOTH_STEP);
            self.scroll_queue.push_back(direction * step);
            remaining -= step;
        }
    }

    fn start_held_scroll(&mut self, direction: i32, now_ms: u64) {
        if self.held_direction != direction {
            if self.scroll_direction != 0 && self.scroll_direction != direction {
                self.scroll_queue.clear();
            }
            self.scroll_direction = direction;
            self.held_started_ms = now_ms;
            self.held_output_magnitude = 0;
        }
        self.held_direction = direction;
    }

    fn reset_held_scroll(&mut self) {
        self.held_direction = 0;
        self.held_started_ms = 0;
        self.held_output_magnitude = 0;
    }

    fn finish_held_scroll(&mut self) -> (i32, i32) {
        let result = (self.held_direction, self.held_output_magnitude);
        self.reset_held_scroll();
        result
    }

    fn next_scroll_delta(&mut self, now_ms: u64) -> Option<i32> {
        if let Some(delta) = self.scroll_queue.pop_front() {
            return Some(delta);
        }
        if self.held_direction != 0 {
            let held_ms = now_ms.saturating_sub(self.held_started_ms);
            if held_ms < HOLD_START_MS {
                return Some(0);
            }
            let held_step = if held_ms >= HOLD_FAST_MS {
                HOLD_FAST_STEP
            } else if held_ms >= HOLD_MID_MS {
                HOLD_MID_STEP
            } else {
                HOLD_SLOW_STEP
            };
            let scaled = (held_step * self.config.scroll_gain / 4).clamp(1, 16);
            self.held_output_magnitude += scaled;
            return Some(self.held_direction * scaled);
        }
        self.scroll_direction = 0;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::build_colmi_packet;

    fn gatt_action(code: u8) -> InputEvent {
        InputEvent::GattPacket(build_colmi_packet(&[0x1D, code]).unwrap().to_vec())
    }

    fn inject_engine() -> MappingEngine {
        MappingEngine::new(MappingConfig {
            scroll_gain: 4,
            inject: true,
        })
    }

    fn outputs_of(kind: impl Fn(&Output) -> bool, items: &[Output]) -> Vec<&Output> {
        items.iter().filter(|item| kind(item)).collect()
    }

    #[test]
    fn debug_mode_logs_only() {
        let mut engine = MappingEngine::new(MappingConfig {
            inject: false,
            ..MappingConfig::default()
        });
        let out = engine.handle(gatt_action(3), 0);
        assert!(out.iter().all(|item| matches!(item, Output::Log(_))));
        let later = engine.tick(20);
        assert!(later.iter().all(|item| matches!(item, Output::Log(_))));
    }

    #[test]
    fn double_and_triple_tap_from_gatt() {
        let mut engine = inject_engine();
        engine.handle(gatt_action(1), 0);
        engine.handle(gatt_action(1), 200);
        let flushed = engine.tick(200 + R08_TAP_FLUSH_MS);
        assert!(flushed.contains(&Output::Copy), "{flushed:?}");

        let mut engine = inject_engine();
        engine.handle(gatt_action(1), 0);
        engine.handle(gatt_action(1), 400);
        engine.handle(gatt_action(1), 800);
        let flushed = engine.tick(800 + R08_TAP_FLUSH_MS);
        assert!(flushed.contains(&Output::Paste), "{flushed:?}");
    }

    #[test]
    fn single_tap_is_noop_and_debounce_collapses_duplicates() {
        let mut engine = inject_engine();
        engine.handle(gatt_action(1), 0);
        engine.handle(gatt_action(1), 40);
        let flushed = engine.tick(R08_TAP_FLUSH_MS);
        assert!(
            flushed
                .iter()
                .any(|item| matches!(item, Output::Log(text) if text.contains("单击"))),
            "{flushed:?}"
        );
        assert!(!flushed
            .iter()
            .any(|item| matches!(item, Output::Copy | Output::Paste)));
    }

    #[test]
    fn action_four_five_and_camera_are_noop() {
        let mut engine = inject_engine();
        for code in [4, 5] {
            let out = engine.handle(gatt_action(code), 0);
            assert!(
                !out.iter().any(|item| matches!(item, Output::Wheel(_))),
                "{out:?}"
            );
        }
        let camera = build_colmi_packet(&[0x02, 0x02]).unwrap().to_vec();
        let out = engine.handle(InputEvent::GattPacket(camera), 0);
        assert!(!out
            .iter()
            .any(|item| matches!(item, Output::Wheel(_) | Output::Copy | Output::Paste)));
    }

    #[test]
    fn gatt_swipe_queues_two_notches_and_reverses_immediately() {
        let mut engine = inject_engine();
        engine.handle(gatt_action(3), 0);
        let mut up = 0;
        for t in (10..400).step_by(10) {
            for item in engine.tick(t) {
                if let Output::Wheel(delta) = item {
                    up += delta;
                }
            }
        }
        assert_eq!(up, GATT_SWIPE_DISTANCE);

        engine.handle(gatt_action(2), 500);
        let first = engine.tick(510);
        assert!(first
            .iter()
            .any(|item| matches!(item, Output::Wheel(delta) if *delta < 0)));
    }

    #[test]
    fn hid_short_swipe_emits_one_notch_and_stops() {
        let mut engine = inject_engine();
        engine.handle(
            InputEvent::HidMouse(HidMouseEvent {
                is_ring: true,
                button_flags: LEFT_BUTTON_DOWN,
                button_data: 0,
                dx: 0,
                dy: 0,
            }),
            0,
        );
        engine.handle(
            InputEvent::HidMouse(HidMouseEvent {
                is_ring: true,
                button_flags: 0,
                button_data: 0,
                dx: 0,
                dy: 4,
            }),
            20,
        );
        let release = engine.handle(
            InputEvent::HidMouse(HidMouseEvent {
                is_ring: true,
                button_flags: LEFT_BUTTON_UP,
                button_data: 0,
                dx: 0,
                dy: 0,
            }),
            80,
        );
        assert!(release
            .iter()
            .any(|item| matches!(item, Output::Log(text) if text.contains("短划"))));
        let mut total = 0;
        for t in (90..400).step_by(10) {
            for item in engine.tick(t) {
                if let Output::Wheel(delta) = item {
                    total += delta;
                }
            }
        }
        assert_eq!(total, -WHEEL_DELTA);
        assert!(engine.tick(500).is_empty());
    }

    #[test]
    fn hid_hold_starts_after_300ms_and_stops_on_release() {
        let mut engine = inject_engine();
        engine.handle(
            InputEvent::HidMouse(HidMouseEvent {
                is_ring: true,
                button_flags: LEFT_BUTTON_DOWN,
                button_data: 0,
                dx: 0,
                dy: 0,
            }),
            0,
        );
        engine.handle(
            InputEvent::HidMouse(HidMouseEvent {
                is_ring: true,
                button_flags: 0,
                button_data: 0,
                dx: 0,
                dy: -3,
            }),
            10,
        );
        let early: i32 = (20..300)
            .step_by(10)
            .flat_map(|t| engine.tick(t))
            .filter_map(|item| match item {
                Output::Wheel(delta) => Some(delta),
                _ => None,
            })
            .sum();
        assert_eq!(early, 0);
        let moving: i32 = (300..360)
            .step_by(10)
            .flat_map(|t| engine.tick(t))
            .filter_map(|item| match item {
                Output::Wheel(delta) => Some(delta),
                _ => None,
            })
            .sum();
        assert!(
            moving > 0,
            "held scroll should emit after 300ms, got {moving}"
        );
        engine.handle(
            InputEvent::HidMouse(HidMouseEvent {
                is_ring: true,
                button_flags: LEFT_BUTTON_UP,
                button_data: 0,
                dx: 0,
                dy: 0,
            }),
            360,
        );
        let after_release: i32 = (370..500)
            .step_by(10)
            .flat_map(|t| engine.tick(t))
            .filter_map(|item| match item {
                Output::Wheel(delta) => Some(delta),
                _ => None,
            })
            .sum();
        // leftover short-swipe remainder may drain, but held generation must stop.
        assert!(after_release.abs() <= WHEEL_DELTA);
        let later: i32 = (800..900)
            .step_by(10)
            .flat_map(|t| engine.tick(t))
            .filter_map(|item| match item {
                Output::Wheel(delta) => Some(delta),
                _ => None,
            })
            .sum();
        assert_eq!(later, 0);
    }

    #[test]
    fn non_ring_mouse_only_updates_cursor_anchor() {
        let mut engine = inject_engine();
        let out = engine.handle(
            InputEvent::HidMouse(HidMouseEvent {
                is_ring: false,
                button_flags: LEFT_BUTTON_DOWN,
                button_data: 0,
                dx: 3,
                dy: 8,
            }),
            0,
        );
        assert_eq!(out, vec![Output::CaptureCursorAnchor]);
    }

    #[test]
    fn gatt_swipe_suppressed_after_hid_vertical() {
        let mut engine = inject_engine();
        engine.handle(
            InputEvent::HidMouse(HidMouseEvent {
                is_ring: true,
                button_flags: LEFT_BUTTON_DOWN,
                button_data: 0,
                dx: 0,
                dy: 0,
            }),
            0,
        );
        engine.handle(
            InputEvent::HidMouse(HidMouseEvent {
                is_ring: true,
                button_flags: 0,
                button_data: 0,
                dx: 0,
                dy: 5,
            }),
            10,
        );
        let out = engine.handle(gatt_action(2), 100);
        assert!(out
            .iter()
            .any(|item| matches!(item, Output::Log(text) if text.contains("忽略离散动作"))));
    }

    #[test]
    fn held_scroll_caps_at_fast_step() {
        let mut engine = inject_engine();
        engine.handle(
            InputEvent::HidMouse(HidMouseEvent {
                is_ring: true,
                button_flags: LEFT_BUTTON_DOWN,
                button_data: 0,
                dx: 0,
                dy: 0,
            }),
            0,
        );
        engine.handle(
            InputEvent::HidMouse(HidMouseEvent {
                is_ring: true,
                button_flags: 0,
                button_data: 0,
                dx: 0,
                dy: -2,
            }),
            0,
        );
        let tick = engine.tick(HOLD_FAST_MS + 10);
        let wheels: Vec<_> = outputs_of(|item| matches!(item, Output::Wheel(_)), &tick);
        assert_eq!(wheels, vec![&Output::Wheel(HOLD_FAST_STEP)]);
    }

    #[test]
    fn changing_control_state_discards_pending_actions() {
        let mut engine = inject_engine();
        engine.handle(gatt_action(3), 0);
        engine.handle(gatt_action(1), 10);
        engine.handle(gatt_action(1), 210);

        engine.set_inject_enabled(false);
        assert!(!engine.inject_enabled());
        assert!(engine.tick(2_000).is_empty());

        engine.set_inject_enabled(true);
        assert!(engine.inject_enabled());
        assert!(engine.tick(2_010).is_empty());
    }
}
