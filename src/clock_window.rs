//! Standalone native clock view. It reads the application's cached usage snapshot;
//! it never performs provider requests of its own.
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use windows::core::PCWSTR;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::native_interop;

const TIMER_CLOCK: usize = 1;
static SELECTED_PROVIDER: AtomicUsize = AtomicUsize::new(1);
static ALWAYS_ON_TOP: AtomicBool = AtomicBool::new(true);
static DIAGNOSTIC_PAINT_COUNT: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub available: bool,
    pub remaining_percent: f64,
    pub refresh_minutes: u32,
    pub status: String,
    pub ahead_of_time: Option<bool>,
    pub starts_at: String,
    pub chinese: bool,
    pub provider_name: String,
    pub weekly: Option<String>,
    pub refresh_state: String,
    pub pace_reason: String,
    pub reset_at: Option<std::time::SystemTime>,
    pub window_seconds: Option<u64>,
}

pub fn selected_provider() -> usize {
    SELECTED_PROVIDER.load(Ordering::Relaxed).min(2)
}

pub fn select_available_provider() {
    let enabled = crate::window::clock_enabled_providers();
    let current = selected_provider();
    if enabled[current] {
        return;
    }
    if let Some(id) = enabled.iter().position(|value| *value) {
        SELECTED_PROVIDER.store(id, Ordering::Relaxed);
    }
}

fn next_provider() {
    let enabled = crate::window::clock_enabled_providers();
    let current = selected_provider();
    for offset in 1..=3 {
        let candidate = (current + offset) % 3;
        if enabled[candidate] {
            SELECTED_PROVIDER.store(candidate, Ordering::Relaxed);
            break;
        }
    }
}

pub fn show() {
    crate::diagnose::log("opening standalone clock window");
    select_available_provider();
    unsafe {
        let class = native_interop::wide_str("CodexUsageClockWindow");
        let instance = match GetModuleHandleW(PCWSTR::null()) {
            Ok(value) => HINSTANCE(value.0),
            Err(_) => return,
        };
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(clock_proc),
            hCursor: LoadCursorW(HINSTANCE::default(), IDC_ARROW).unwrap_or_default(),
            hbrBackground: HBRUSH::default(),
            hInstance: instance,
            lpszClassName: PCWSTR::from_raw(class.as_ptr()),
            ..Default::default()
        };
        let _ = RegisterClassExW(&wc);
        let title = native_interop::wide_str("Codex Usage Clock");
        if let Some(existing) = FindWindowW(
            PCWSTR::from_raw(class.as_ptr()),
            PCWSTR::from_raw(title.as_ptr()),
        )
        .ok()
        .filter(|hwnd| !hwnd.is_invalid())
        {
            let _ = SetTimer(existing, TIMER_CLOCK, 1000, None);
            let _ = ShowWindow(existing, SW_SHOWNORMAL);
            let _ = SetForegroundWindow(existing);
            return;
        }
        let dpi = GetDpiForSystem().max(96) as i32;
        let hwnd = CreateWindowExW(
            WS_EX_APPWINDOW,
            PCWSTR::from_raw(class.as_ptr()),
            PCWSTR::from_raw(title.as_ptr()),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            scale(dpi, 1200),
            scale(dpi, 850),
            HWND::default(),
            HMENU::default(),
            instance,
            None,
        );
        if let Ok(hwnd) = hwnd {
            // Explicitly show the view even when the launcher used a hidden
            // startup window (the tray host itself is intentionally hidden).
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
            let _ = SetTimer(hwnd, TIMER_CLOCK, 1000, None);
            let _ = SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
            crate::diagnose::log(format!("standalone clock window shown hwnd={:?}", hwnd));
        } else if let Err(error) = hwnd {
            crate::diagnose::log_error("unable to create standalone clock window", error);
        }
    }
}

unsafe fn show_refresh_menu(hwnd: HWND) {
    let Ok(menu) = CreatePopupMenu() else {
        return;
    };
    let snapshot = crate::window::clock_snapshot(selected_provider());
    for minutes in [1, 5, 15, 60] {
        let label = native_interop::wide_str(&if snapshot.chinese {
            format!("每 {minutes} 分钟")
        } else {
            format!("Every {minutes} min")
        });
        let flags = MF_STRING
            | if snapshot.refresh_minutes == minutes {
                MF_CHECKED
            } else {
                MF_UNCHECKED
            };
        let _ = AppendMenuW(menu, flags, minutes as usize, PCWSTR(label.as_ptr()));
    }
    let mut cursor = POINT::default();
    let _ = GetCursorPos(&mut cursor);
    let selected = TrackPopupMenu(
        menu,
        TPM_RETURNCMD | TPM_NONOTIFY,
        cursor.x,
        cursor.y,
        0,
        hwnd,
        None,
    )
    .0;
    let _ = DestroyMenu(menu);
    if selected > 0 {
        crate::window::clock_refresh_command(Some(selected as u32));
    }
}

unsafe extern "system" fn clock_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_SIZE => {
            if wparam.0 == SIZE_MINIMIZED as usize {
                let _ = KillTimer(hwnd, TIMER_CLOCK);
            } else {
                let _ = SetTimer(hwnd, TIMER_CLOCK, 1000, None);
                let _ = InvalidateRect(hwnd, None, BOOL(0));
            }
            LRESULT(0)
        }
        WM_SHOWWINDOW => {
            if wparam.0 == 0 {
                let _ = KillTimer(hwnd, TIMER_CLOCK);
            } else if !IsIconic(hwnd).as_bool() {
                let _ = SetTimer(hwnd, TIMER_CLOCK, 1000, None);
            }
            LRESULT(0)
        }
        WM_TIMER => {
            let _ = InvalidateRect(hwnd, None, BOOL(0));
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = KillTimer(hwnd, TIMER_CLOCK);
            let _ = ShowWindow(hwnd, SW_HIDE);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let mut rect = RECT::default();
            let _ = GetClientRect(hwnd, &mut rect);
            let zoom = (f64::from(rect.right) / 1180.0).min(f64::from(rect.bottom) / 800.0);
            let x = f64::from(lparam.0 as u16 as i16) / zoom
                - (f64::from(rect.right) / zoom - 1180.0) / 2.0;
            let y = f64::from((lparam.0 >> 16) as u16 as i16) / zoom
                - (f64::from(rect.bottom) / zoom - 800.0) / 2.0;
            if (630.0..=659.0).contains(&y) && (750.0..=905.0).contains(&x) {
                show_refresh_menu(hwnd);
            } else if (630.0..=659.0).contains(&y) && (972.0..=1092.0).contains(&x) {
                crate::window::clock_refresh_command(None);
            } else {
                next_provider();
            }
            let _ = InvalidateRect(hwnd, None, BOOL(0));
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            let topmost = !ALWAYS_ON_TOP.fetch_xor(true, Ordering::Relaxed);
            let insert_after = if topmost {
                HWND_TOPMOST
            } else {
                HWND_NOTOPMOST
            };
            let _ = SetWindowPos(
                hwnd,
                insert_after,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
            let _ = InvalidateRect(hwnd, None, BOOL(0));
            LRESULT(0)
        }
        0x02E0 => {
            let suggested = &*(lparam.0 as *const RECT);
            let _ = SetWindowPos(
                hwnd,
                HWND::default(),
                suggested.left,
                suggested.top,
                suggested.right - suggested.left,
                suggested.bottom - suggested.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            let _ = InvalidateRect(hwnd, None, BOOL(0));
            LRESULT(0)
        }
        WM_GETMINMAXINFO => {
            let limits = &mut *(lparam.0 as *mut MINMAXINFO);
            let dpi = GetDpiForWindow(hwnd).max(96) as i32;
            limits.ptMinTrackSize.x = scale(dpi, 900);
            limits.ptMinTrackSize.y = scale(dpi, 650);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            let paint_number = DIAGNOSTIC_PAINT_COUNT.fetch_add(1, Ordering::Relaxed);
            let trace_paint = crate::diagnose::is_enabled() && paint_number < 20;
            if trace_paint {
                crate::diagnose::log(format!(
                    "standalone clock paint {} started",
                    paint_number + 1
                ));
            }
            paint(hwnd);
            if trace_paint {
                crate::diagnose::log(format!(
                    "standalone clock paint {} finished",
                    paint_number + 1
                ));
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            let _ = KillTimer(hwnd, TIMER_CLOCK);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

// Render in the prototype's logical coordinate space, then downsample the
// offscreen surface. Text, curves, and ticks share one scale at every DPI.
const BG: u32 = 0x080b10;
const INK: u32 = 0xeef3f8;
const MUTED: u32 = 0x8996a1;
const CYAN: u32 = 0x9cecf0;
const GREEN: u32 = 0x65e6ab;
const RED: u32 = 0xff707b;
const ORANGE: u32 = 0xffb565;

fn rgb(hex: u32) -> COLORREF {
    COLORREF(((hex & 255) << 16) | (hex & 0xff00) | ((hex >> 16) & 255))
}

struct Canvas {
    dc: HDC,
    zoom: f64,
    ox: f64,
    oy: f64,
}
impl Canvas {
    fn x(&self, n: f64) -> i32 {
        ((n + self.ox) * self.zoom).round() as i32
    }
    fn y(&self, n: f64) -> i32 {
        ((n + self.oy) * self.zoom).round() as i32
    }
    fn d(&self, n: f64) -> i32 {
        (n * self.zoom).round().max(1.0) as i32
    }
    unsafe fn panel(&self, x: f64, y: f64, w: f64, h: f64, r: f64, fill: u32, border: u32) {
        let brush = CreateSolidBrush(rgb(fill));
        let pen = CreatePen(PS_SOLID, self.d(1.0), rgb(border));
        let ob = SelectObject(self.dc, brush);
        let op = SelectObject(self.dc, pen);
        let _ = RoundRect(
            self.dc,
            self.x(x),
            self.y(y),
            self.x(x + w),
            self.y(y + h),
            self.d(r * 2.0),
            self.d(r * 2.0),
        );
        let _ = SelectObject(self.dc, ob);
        let _ = SelectObject(self.dc, op);
        let _ = DeleteObject(brush);
        let _ = DeleteObject(pen);
    }
    unsafe fn line(&self, a: (f64, f64), b: (f64, f64), weight: f64, color: u32) {
        let pen = CreatePen(PS_SOLID, self.d(weight), rgb(color));
        let old = SelectObject(self.dc, pen);
        let _ = MoveToEx(self.dc, self.x(a.0), self.y(a.1), None);
        let _ = LineTo(self.dc, self.x(b.0), self.y(b.1));
        let _ = SelectObject(self.dc, old);
        let _ = DeleteObject(pen);
    }
    unsafe fn text(
        &self,
        text: &str,
        x: f64,
        y: f64,
        w: f64,
        size: f64,
        color: u32,
        bold: bool,
        center: bool,
    ) {
        if text.is_empty() {
            return;
        }
        let face = native_interop::wide_str(if text.is_ascii() {
            "Segoe UI"
        } else {
            "Microsoft YaHei UI"
        });
        let font = CreateFontW(
            -self.d(size),
            0,
            0,
            0,
            if bold { 600 } else { 400 },
            0,
            0,
            0,
            DEFAULT_CHARSET.0 as u32,
            OUT_TT_PRECIS.0 as u32,
            CLIP_DEFAULT_PRECIS.0 as u32,
            ANTIALIASED_QUALITY.0 as u32,
            0,
            PCWSTR(face.as_ptr()),
        );
        let old = SelectObject(self.dc, font);
        let _ = SetBkMode(self.dc, TRANSPARENT);
        let _ = SetTextColor(self.dc, rgb(color));
        let mut chars: Vec<u16> = text.encode_utf16().collect();
        let mut rect = RECT {
            left: self.x(x),
            top: self.y(y),
            right: self.x(x + w),
            bottom: self.y(y + size * 1.6),
        };
        let _ = DrawTextW(
            self.dc,
            &mut chars,
            &mut rect,
            DT_SINGLELINE
                | DT_NOPREFIX
                | DT_END_ELLIPSIS
                | if center { DT_CENTER } else { DT_LEFT },
        );
        let _ = SelectObject(self.dc, old);
        let _ = DeleteObject(font);
    }
    unsafe fn arc(
        &self,
        cx: f64,
        cy: f64,
        r: f64,
        start: f64,
        sweep: f64,
        weight: f64,
        color: u32,
    ) {
        if sweep.abs() < 0.01 {
            return;
        }
        let steps = (sweep.abs() * 2.0).ceil().max(2.0) as usize;
        let pts: Vec<POINT> = (0..=steps)
            .map(|i| {
                let (x, y) = point(cx, cy, r, start + sweep * i as f64 / steps as f64);
                POINT {
                    x: self.x(x),
                    y: self.y(y),
                }
            })
            .collect();
        let pen = CreatePen(PS_SOLID, self.d(weight), rgb(color));
        let old = SelectObject(self.dc, pen);
        let _ = Polyline(self.dc, &pts);
        let _ = SelectObject(self.dc, old);
        let _ = DeleteObject(pen);
    }
    unsafe fn sector(&self, cx: f64, cy: f64, r: f64, start: f64, sweep: f64, color: u32) {
        let mut pts = vec![POINT {
            x: self.x(cx),
            y: self.y(cy),
        }];
        for i in 0..=100 {
            let (x, y) = point(cx, cy, r, start + sweep * i as f64 / 100.0);
            pts.push(POINT {
                x: self.x(x),
                y: self.y(y),
            });
        }
        let brush = CreateSolidBrush(rgb(color));
        let old = SelectObject(self.dc, brush);
        let op = SelectObject(self.dc, GetStockObject(NULL_PEN));
        let _ = Polygon(self.dc, &pts);
        let _ = SelectObject(self.dc, old);
        let _ = SelectObject(self.dc, op);
        let _ = DeleteObject(brush);
    }
    unsafe fn curved_text(
        &self,
        text: &str,
        cx: f64,
        cy: f64,
        r: f64,
        angle: f64,
        size: f64,
        color: u32,
    ) {
        // Place each glyph on the same circular baseline as SVG textPath.
        let bottom = angle.to_radians().sin() > 0.0;
        let chars: Vec<char> = text.chars().collect();
        let advances: Vec<f64> = chars
            .iter()
            .map(|c| if c.is_ascii() { size * 0.58 } else { size })
            .collect();
        let total: f64 = advances.iter().sum();
        let mut offset = -total / 2.0;
        for (ch, advance) in chars.iter().zip(advances.iter()) {
            let a = angle
                + if bottom { -1.0 } else { 1.0 } * (offset + advance / 2.0) / r * 180.0
                    / std::f64::consts::PI;
            let (x, y) = point(cx, cy, r, a);
            let rotation = if bottom { 90.0 - a } else { -90.0 - a };
            let face = native_interop::wide_str("Microsoft YaHei UI");
            let font = CreateFontW(
                -self.d(size),
                0,
                (rotation * 10.0) as i32,
                (rotation * 10.0) as i32,
                500,
                0,
                0,
                0,
                DEFAULT_CHARSET.0 as u32,
                OUT_TT_PRECIS.0 as u32,
                CLIP_DEFAULT_PRECIS.0 as u32,
                ANTIALIASED_QUALITY.0 as u32,
                0,
                PCWSTR(face.as_ptr()),
            );
            let old = SelectObject(self.dc, font);
            let _ = SetTextColor(self.dc, rgb(color));
            let previous = SetTextAlign(self.dc, TA_CENTER | TA_BASELINE);
            let glyph: Vec<u16> = ch.to_string().encode_utf16().collect();
            let _ = TextOutW(self.dc, self.x(x), self.y(y), &glyph);
            let _ = SetTextAlign(self.dc, TEXT_ALIGN_OPTIONS(previous));
            let _ = SelectObject(self.dc, old);
            let _ = DeleteObject(font);
            offset += advance;
        }
    }
    unsafe fn tag(
        &self,
        cx: f64,
        cy: f64,
        angle: f64,
        label: &str,
        value: &str,
        color: u32,
        surface: u32,
    ) {
        self.arc(cx, cy, 238.0, angle - 17.0, 34.0, 48.0, color);
        self.arc(cx, cy, 238.0, angle - 17.0, 34.0, 46.0, surface);
        let bottom = angle.to_radians().sin() > 0.0;
        self.curved_text(
            label,
            cx,
            cy,
            if bottom { 224.0 } else { 247.0 },
            angle,
            10.0,
            MUTED,
        );
        self.curved_text(
            value,
            cx,
            cy,
            if bottom { 247.0 } else { 229.0 },
            angle,
            16.0,
            color,
        );
    }
}
fn point(cx: f64, cy: f64, r: f64, a: f64) -> (f64, f64) {
    let a = a.to_radians();
    (cx + r * a.cos(), cy + r * a.sin())
}
fn clock_angle(time: std::time::SystemTime) -> Option<f64> {
    native_interop::system_time_to_local(time).map(|t| {
        -90.0
            + (f64::from(t.wHour % 12)
                + f64::from(t.wMinute) / 60.0
                + f64::from(t.wSecond) / 3600.0)
                * 30.0
    })
}
fn time_label(time: std::time::SystemTime) -> String {
    native_interop::system_time_to_local(time)
        .map(|t| format!("{:02}:{:02}", t.wHour, t.wMinute))
        .unwrap_or_else(|| "--:--".into())
}
// Window and quota angles use the provider's declared duration. Unknown duration
// or stale data never acquires an invented five-hour arc or a green safety band.
fn window_geometry(
    s: &Snapshot,
    now: std::time::SystemTime,
) -> Option<(std::time::SystemTime, f64, f64, f64)> {
    let duration = std::time::Duration::from_secs(s.window_seconds.filter(|n| *n > 0)?);
    let reset = s.reset_at?;
    let start = reset.checked_sub(duration)?;
    let left = reset.duration_since(now).ok()?;
    if !s.available || left > duration || !s.remaining_percent.is_finite() {
        return None;
    }
    let elapsed = 1.0 - left.as_secs_f64() / duration.as_secs_f64();
    let sweep = (duration.as_secs_f64() / 43200.0 * 360.0).min(359.9);
    Some((
        start,
        elapsed,
        sweep,
        s.remaining_percent - left.as_secs_f64() / duration.as_secs_f64() * 100.0,
    ))
}
unsafe fn scene(c: &Canvas, s: &Snapshot, now: std::time::SystemTime) {
    let zh = s.chinese;
    let tr = |a, b| if zh { a } else { b };
    c.panel(0.0, 0.0, 1180.0, 800.0, 0.0, BG, BG);
    c.panel(30.0, 23.0, 31.0, 31.0, 10.0, 0x11232b, 0x31535c);
    c.text("◷", 30.0, 23.0, 31.0, 22.0, CYAN, false, true);
    c.text("CODEX", 72.0, 30.0, 67.0, 12.0, INK, true, false);
    c.text("PACE", 133.0, 30.0, 80.0, 12.0, CYAN, true, false);
    c.text(
        &format!(
            "●  {} · {}",
            s.provider_name,
            tr("本机实时额度", "Local quota cache")
        ),
        780.0,
        30.0,
        365.0,
        12.0,
        MUTED,
        false,
        false,
    );
    c.panel(28.0, 77.0, 1124.0, 692.0, 28.0, 0x10161e, 0x222a33);
    c.text(
        tr("额度节奏", "QUOTA PACE"),
        65.0,
        112.0,
        270.0,
        11.0,
        MUTED,
        false,
        false,
    );
    c.text(
        tr("跟上重置节拍", "Keep pace with your reset"),
        65.0,
        132.0,
        430.0,
        26.0,
        INK,
        true,
        false,
    );
    let chip = s
        .window_seconds
        .filter(|n| *n > 0)
        .map(|n| format!("●  {}H WINDOW", n as f64 / 3600.0))
        .unwrap_or_else(|| "SESSION WINDOW".into());
    c.panel(461.0, 135.0, 135.0, 29.0, 15.0, 0x14232b, 0x293d47);
    c.text(&chip, 466.0, 141.0, 125.0, 10.0, CYAN, false, true);
    let (cx, cy) = (328.0, 407.0);
    let r = 168.0;
    c.panel(
        cx - 179.0,
        cy - 179.0,
        358.0,
        358.0,
        179.0,
        0x0a1016,
        0x1d262e,
    );
    let geometry = window_geometry(s, now);
    let mut pace_color = CYAN;
    let mut diff = None;
    if let Some((start, elapsed, sweep, delta)) = geometry {
        let a = clock_angle(start).unwrap_or(-90.0);
        let quota = a + (1.0 - s.remaining_percent / 100.0) * sweep;
        let time = a + elapsed * sweep;
        c.arc(cx, cy, 189.0, a, sweep, 10.0, 0x344855);
        c.arc(cx, cy, 189.0, a, sweep, 4.0, 0x49646c);
        if s.ahead_of_time.is_some() {
            pace_color = if delta > 0.0 {
                GREEN
            } else if delta < 0.0 {
                RED
            } else {
                CYAN
            };
            diff = Some(delta);
            c.sector(
                cx,
                cy,
                r,
                quota.min(time),
                (quota - time).abs(),
                if delta >= 0.0 { 0x17372d } else { 0x3a202b },
            );
            c.arc(
                cx,
                cy,
                189.0,
                quota.min(time),
                (quota - time).abs(),
                15.0,
                pace_color,
            );
        }
        for (angle, color, inner, outer) in [
            (a, CYAN, 201.0, 215.0),
            (a + sweep, ORANGE, 201.0, 215.0),
            (time, CYAN, 181.0, 203.0),
            (quota, INK, 181.0, 213.0),
        ] {
            c.line(
                point(cx, cy, inner, angle),
                point(cx, cy, outer, angle),
                2.5,
                color,
            );
        }
        let separate = |angle: f64, direction: f64| {
            let gap = (angle - quota).abs().rem_euclid(360.0);
            let gap = gap.min(360.0 - gap);
            angle + direction * (50.0 - gap).max(0.0)
        };
        let start_label = separate(a, -1.0);
        let end_label = separate(a + sweep, 1.0);
        for (exact, label, color) in [(a, start_label, CYAN), (a + sweep, end_label, ORANGE)] {
            c.arc(
                cx,
                cy,
                210.0,
                exact.min(label),
                (exact - label).abs(),
                1.0,
                color,
            );
        }
        c.tag(
            cx,
            cy,
            start_label,
            tr("推导起点", "DERIVED START"),
            &time_label(start),
            CYAN,
            0x101c26,
        );
        c.tag(
            cx,
            cy,
            end_label,
            tr("额度重置", "QUOTA RESET"),
            &s.reset_at.map(time_label).unwrap_or_default(),
            ORANGE,
            0x101c26,
        );
        c.tag(
            cx,
            cy,
            quota,
            tr("剩余额度", "REMAINING"),
            &format!("{:.0}%", s.remaining_percent),
            pace_color,
            if diff.is_some() {
                if delta >= 0.0 {
                    0x10291f
                } else {
                    0x2b1720
                }
            } else {
                0x101c26
            },
        );
        if s.reset_at
            .and_then(|reset| reset.duration_since(now).ok())
            .is_some_and(|d| d.as_secs() <= 3600)
        {
            if let (Some(t), Some(reset)) = (
                native_interop::system_time_to_local(now),
                s.reset_at.and_then(native_interop::system_time_to_local),
            ) {
                let a = -90.0 + (f64::from(t.wMinute) + f64::from(t.wSecond) / 60.0) * 6.0;
                let b = -90.0 + (f64::from(reset.wMinute) + f64::from(reset.wSecond) / 60.0) * 6.0;
                c.arc(cx, cy, 142.0, a, (b - a).rem_euclid(360.0), 2.5, 0xe1ffff);
                c.line(
                    point(cx, cy, 135.0, b),
                    point(cx, cy, 152.0, b),
                    3.0,
                    0xe1ffff,
                );
            }
        }
    }
    for i in 0..60 {
        let a = -90.0 + f64::from(i) * 6.0;
        let major = i % 5 == 0;
        c.line(
            point(cx, cy, if major { 152.0 } else { 158.0 }, a),
            point(cx, cy, r, a),
            if i % 15 == 0 {
                4.0
            } else if major {
                2.8
            } else {
                2.0
            },
            if major { 0xdce5ec } else { 0x35414c },
        );
    }
    for hour in 1..=12 {
        let (x, y) = point(cx, cy, 128.0, -90.0 + f64::from(hour) * 30.0);
        c.text(
            &hour.to_string(),
            x - 21.0,
            y - 16.0,
            42.0,
            23.0,
            INK,
            true,
            true,
        );
    }
    if let Some(t) = native_interop::system_time_to_local(now) {
        let sec = f64::from(t.wSecond);
        let min = f64::from(t.wMinute) + sec / 60.0;
        let hour = f64::from(t.wHour % 12) + min / 60.0;
        for (a, len, tail, weight, color) in [
            (hour * 30.0 - 90.0, 77.0, 15.0, 6.0, INK),
            (min * 6.0 - 90.0, 111.0, 18.0, 2.6, INK),
            (sec * 6.0 - 90.0, 135.0, 22.0, 1.4, CYAN),
        ] {
            c.line(
                point(cx, cy, tail, a + 180.0),
                point(cx, cy, len, a),
                weight,
                color,
            );
        }
        c.text(
            &format!("{:02}:{:02}:{:02}", t.wHour, t.wMinute, t.wSecond),
            170.0,
            703.0,
            316.0,
            33.0,
            INK,
            false,
            true,
        );
    }
    c.panel(cx - 7.0, cy - 7.0, 14.0, 14.0, 7.0, 0x0b1117, ORANGE);
    c.panel(cx - 3.0, cy - 3.0, 6.0, 6.0, 3.0, INK, INK);
    c.text(
        tr("本机时间", "LOCAL TIME"),
        170.0,
        682.0,
        316.0,
        10.0,
        MUTED,
        false,
        true,
    );
    let legends = [
        (CYAN, tr("时间走到", "Time")),
        (INK, tr("额度用到", "Quota")),
        (GREEN, tr("用得慢", "Slower")),
        (RED, tr("用得快", "Faster")),
    ];
    for (i, (color, label)) in legends.iter().enumerate() {
        let x = 157.0 + i as f64 * 92.0;
        c.panel(x, 750.0, 5.0, 5.0, 2.5, *color, *color);
        c.text(label, x + 12.0, 744.0, 80.0, 10.0, MUTED, false, false);
    }
    let (card_bg, card_border) = match diff {
        Some(d) if d >= 0.0 => (0x123027, 0x2b6250),
        Some(_) => (0x311c26, 0x67313c),
        None => (0x111b24, 0x293b46),
    };
    c.panel(657.0, 121.0, 456.0, 224.0, 17.0, card_bg, card_border);
    c.text("PACE CHECK", 679.0, 141.0, 240.0, 10.0, MUTED, false, false);
    c.panel(973.0, 137.0, 119.0, 27.0, 14.0, card_bg, card_border);
    c.text(
        if diff.is_some() {
            if pace_color == GREEN {
                tr("用得慢 · 绿色", "Slower · green")
            } else if pace_color == CYAN {
                tr("进度一致", "In sync")
            } else {
                tr("用得快 · 红色", "Faster · red")
            }
        } else {
            tr("等待有效窗口", "Awaiting window")
        },
        977.0,
        143.0,
        111.0,
        10.0,
        pace_color,
        false,
        true,
    );
    c.text(
        &diff
            .map(|d| format!("{:+.1} pp", d))
            .unwrap_or_else(|| "-- pp".into()),
        679.0,
        173.0,
        190.0,
        36.0,
        pace_color,
        false,
        false,
    );
    c.text(
        match diff {
            Some(d) if d.abs() < 1e-7 => tr("额度消耗与时间同步", "Quota use matches time"),
            Some(d) if d > 0.0 => tr("额度消耗落后于时间", "Quota use is behind time"),
            Some(_) => tr("额度消耗领先于时间", "Quota use is ahead of time"),
            None => tr("暂停节奏比较", "Pace comparison paused"),
        },
        870.0,
        191.0,
        225.0,
        12.0,
        INK,
        true,
        false,
    );
    c.text(
        if diff.is_some() {
            tr(
                "比较同一窗口内的时间推进与额度消耗。",
                "Compare time elapsed and quota used in this window.",
            )
        } else {
            &s.pace_reason
        },
        679.0,
        226.0,
        412.0,
        11.0,
        MUTED,
        false,
        false,
    );
    c.line((679.0, 255.0), (1091.0, 255.0), 1.0, 0x32453f);
    for (i, label) in [tr("时间已走", "Time elapsed"), tr("额度已用", "Quota used")]
        .iter()
        .enumerate()
    {
        let y = 274.0 + i as f64 * 29.0;
        let value = if i == 0 {
            geometry.map(|g| g.1 * 100.0)
        } else {
            if s.available {
                Some(100.0 - s.remaining_percent)
            } else {
                None
            }
        };
        c.text(label, 679.0, y - 4.0, 79.0, 10.0, INK, false, false);
        c.panel(761.0, y, 268.0, 9.0, 4.5, 0x081410, 0x293d35);
        if let Some(value) = value {
            if value > 0.0 {
                c.panel(
                    761.0,
                    y,
                    268.0 * value.clamp(0.0, 100.0) / 100.0,
                    9.0,
                    4.5,
                    if i == 0 { CYAN } else { pace_color },
                    if i == 0 { CYAN } else { pace_color },
                );
            }
        }
        c.text(
            &value
                .map(|v| format!("{:.1}%", v))
                .unwrap_or_else(|| "--".into()),
            1036.0,
            y - 4.0,
            60.0,
            10.0,
            INK,
            false,
            true,
        );
    }
    for (i, label) in [
        tr("剩余额度", "Remaining quota"),
        tr("重置倒计时", "Resets in"),
    ]
    .iter()
    .enumerate()
    {
        let x = 657.0 + i as f64 * 235.0;
        c.panel(x, 359.0, 221.0, 117.0, 17.0, 0x111821, 0x232c35);
        c.text(label, x + 18.0, 375.0, 190.0, 11.0, MUTED, false, false);
        let val = if i == 0 {
            if s.available {
                format!("{:.0}%", s.remaining_percent)
            } else {
                "--".into()
            }
        } else {
            s.reset_at
                .and_then(|t| t.duration_since(now).ok())
                .map(|d| {
                    format!(
                        "{:02}:{:02}:{:02}",
                        d.as_secs() / 3600,
                        d.as_secs() % 3600 / 60,
                        d.as_secs() % 60
                    )
                })
                .unwrap_or_else(|| "--:--:--".into())
        };
        c.text(
            &val,
            x + 18.0,
            400.0,
            190.0,
            if i == 0 { 30.0 } else { 27.0 },
            INK,
            false,
            false,
        );
        c.text(
            &if i == 0 {
                tr("当前会话窗口", "Current session window").into()
            } else {
                format!(
                    "{} {}",
                    tr("重置时间", "Reset at"),
                    s.reset_at.map(time_label).unwrap_or_else(|| "--:--".into())
                )
            },
            x + 18.0,
            449.0,
            190.0,
            10.0,
            MUTED,
            false,
            false,
        );
    }
    c.panel(657.0, 490.0, 456.0, 238.0, 17.0, 0x111821, 0x232c35);
    c.text(
        "LIVE SOURCE",
        677.0,
        509.0,
        300.0,
        10.0,
        MUTED,
        false,
        false,
    );
    c.text(
        &format!("{} {}", s.provider_name, tr("实时额度", "live quota")),
        677.0,
        530.0,
        410.0,
        17.0,
        INK,
        true,
        false,
    );
    c.panel(677.0, 569.0, 416.0, 54.0, 11.0, 0x0d141c, 0x222c35);
    c.text("↻", 689.0, 581.0, 27.0, 22.0, CYAN, false, true);
    c.text(
        tr(
            "本轮窗口 · 推导起点 — 重置",
            "Window · derived start — reset",
        ),
        728.0,
        578.0,
        350.0,
        10.0,
        MUTED,
        false,
        false,
    );
    c.text(
        &format!(
            "{} — {}",
            s.starts_at,
            s.reset_at.map(time_label).unwrap_or_else(|| "--:--".into())
        ),
        728.0,
        596.0,
        350.0,
        12.0,
        INK,
        false,
        false,
    );
    c.text(
        tr("自动刷新", "Auto refresh"),
        678.0,
        639.0,
        70.0,
        11.0,
        MUTED,
        false,
        false,
    );
    c.panel(750.0, 630.0, 155.0, 29.0, 8.0, 0x101e27, 0x33505a);
    c.text(
        &format!(
            "{} {} {}  ▾",
            tr("每", "Every"),
            s.refresh_minutes.max(1),
            tr("分钟", "min")
        ),
        756.0,
        637.0,
        143.0,
        11.0,
        CYAN,
        false,
        true,
    );
    c.panel(972.0, 630.0, 120.0, 29.0, 8.0, 0x101e27, 0x33505a);
    c.text(
        tr("立即刷新", "Refresh now"),
        976.0,
        637.0,
        112.0,
        11.0,
        CYAN,
        false,
        true,
    );
    c.text(
        &s.refresh_state,
        678.0,
        667.0,
        414.0,
        11.0,
        MUTED,
        false,
        false,
    );
    c.text(
        s.weekly.as_deref().unwrap_or(""),
        678.0,
        690.0,
        414.0,
        11.0,
        MUTED,
        false,
        false,
    );
    c.text(&s.status, 678.0, 709.0, 414.0, 10.0, ORANGE, false, false);
    c.text(
        tr(
            "左键切换服务 · 右键切换置顶",
            "Left click: switch service · Right click: pin window",
        ),
        660.0,
        744.0,
        451.0,
        10.0,
        MUTED,
        false,
        false,
    );
}

unsafe fn paint(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let target = BeginPaint(hwnd, &mut ps);
    let mut rect = RECT::default();
    let _ = GetClientRect(hwnd, &mut rect);
    let (w, h) = (rect.right, rect.bottom);
    if w <= 0 || h <= 0 {
        let _ = EndPaint(hwnd, &ps);
        return;
    }
    let dc = CreateCompatibleDC(target);
    let bitmap = CreateCompatibleBitmap(target, w * 2, h * 2);
    if dc.is_invalid() || bitmap.is_invalid() {
        if !dc.is_invalid() {
            let _ = DeleteDC(dc);
        }
        let _ = EndPaint(hwnd, &ps);
        return;
    }
    let old = SelectObject(dc, bitmap);
    let brush = CreateSolidBrush(rgb(BG));
    let _ = FillRect(
        dc,
        &RECT {
            left: 0,
            top: 0,
            right: w * 2,
            bottom: h * 2,
        },
        brush,
    );
    let _ = DeleteObject(brush);
    let zoom = (f64::from(w) / 1180.0).min(f64::from(h) / 800.0);
    let canvas = Canvas {
        dc,
        zoom: zoom * 2.0,
        ox: (f64::from(w) / zoom - 1180.0) / 2.0,
        oy: (f64::from(h) / zoom - 800.0) / 2.0,
    };
    select_available_provider();
    scene(
        &canvas,
        &crate::window::clock_snapshot(selected_provider()),
        std::time::SystemTime::now(),
    );
    let _ = SetStretchBltMode(target, HALFTONE);
    let _ = SetBrushOrgEx(target, 0, 0, None);
    let _ = StretchBlt(target, 0, 0, w, h, dc, 0, 0, w * 2, h * 2, SRCCOPY);
    let _ = SelectObject(dc, old);
    let _ = DeleteObject(bitmap);
    let _ = DeleteDC(dc);
    let _ = EndPaint(hwnd, &ps);
}
fn scale(dpi: i32, value: i32) -> i32 {
    (value as f64 * dpi as f64 / 96.0).round() as i32
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample(now: std::time::SystemTime, remaining: f64) -> Snapshot {
        Snapshot {
            available: true,
            remaining_percent: remaining,
            reset_at: Some(now + std::time::Duration::from_secs(8100)),
            window_seconds: Some(18000),
            ahead_of_time: Some(remaining >= 45.0),
            chinese: true,
            provider_name: "Codex".into(),
            starts_at: time_label(now - std::time::Duration::from_secs(9900)),
            refresh_state: "上次成功 19:20 · 下次检查 1m 后".into(),
            weekly: Some("每周剩余 72% · 3d 2h 后重置".into()),
            ..Default::default()
        }
    }
    #[test]
    fn geometry_uses_declared_window_and_absolute_elapsed_time() {
        let now = std::time::SystemTime::now();
        let mut s = sample(now, 63.0);
        let (_, elapsed, sweep, delta) = window_geometry(&s, now).unwrap();
        assert!((elapsed - 0.55).abs() < 1e-9);
        assert!((sweep - 150.0).abs() < 1e-9);
        assert!((delta - 18.0).abs() < 1e-9);
        s.window_seconds = None;
        assert!(window_geometry(&s, now).is_none());
        s.window_seconds = Some(18000);
        s.reset_at = Some(now - std::time::Duration::from_secs(1));
        assert!(window_geometry(&s, now).is_none());
        s.reset_at = Some(now + std::time::Duration::from_secs(18001));
        assert!(window_geometry(&s, now).is_none());
    }
    #[test]
    #[ignore = "writes offscreen visual review artifacts into target"]
    fn render_prototype_visual_fixtures() {
        unsafe {
            let now = std::time::SystemTime::now();
            for (name, s) in [
                ("safe", sample(now, 63.0)),
                ("risk", sample(now, 20.0)),
                (
                    "waiting",
                    Snapshot {
                        chinese: true,
                        provider_name: "Codex".into(),
                        pace_reason: "等待有效额度与窗口时长".into(),
                        ..Default::default()
                    },
                ),
            ] {
                let dc = CreateCompatibleDC(HDC::default());
                let info = BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: 1180,
                        biHeight: -800,
                        biPlanes: 1,
                        biBitCount: 32,
                        biCompression: BI_RGB.0,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                let mut bits = std::ptr::null_mut();
                let bmp =
                    CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, HANDLE::default(), 0)
                        .unwrap();
                let old = SelectObject(dc, bmp);
                scene(
                    &Canvas {
                        dc,
                        zoom: 1.0,
                        ox: 0.0,
                        oy: 0.0,
                    },
                    &s,
                    now,
                );
                let _ = GdiFlush();
                let pixels = std::slice::from_raw_parts(bits as *const u8, 1180 * 800 * 4);
                let mut bytes = Vec::new();
                bytes.extend_from_slice(b"BM");
                bytes.extend_from_slice(&(54u32 + pixels.len() as u32).to_le_bytes());
                bytes.extend_from_slice(&[0; 4]);
                bytes.extend_from_slice(&54u32.to_le_bytes());
                bytes.extend_from_slice(&40u32.to_le_bytes());
                bytes.extend_from_slice(&1180i32.to_le_bytes());
                bytes.extend_from_slice(&(-800i32).to_le_bytes());
                bytes.extend_from_slice(&1u16.to_le_bytes());
                bytes.extend_from_slice(&32u16.to_le_bytes());
                bytes.extend_from_slice(&[0; 24]);
                bytes.extend_from_slice(pixels);
                std::fs::create_dir_all("target/clock-review").unwrap();
                std::fs::write(format!("target/clock-review/{name}.bmp"), bytes).unwrap();
                let _ = SelectObject(dc, old);
                let _ = DeleteObject(bmp);
                let _ = DeleteDC(dc);
            }
        }
    }
}
