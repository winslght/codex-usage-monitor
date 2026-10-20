//! Standalone native clock view. It reads the application's cached usage snapshot;
//! it never performs provider requests of its own.
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use windows::core::PCWSTR;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::clock_palette::{self, blend, Palette};
use crate::native_interop;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
};

const TIMER_CLOCK: usize = 1;
const TIMER_MOTION: usize = 2;
#[cfg(test)]
static TEST_SNAPSHOT: Mutex<Option<Snapshot>> = Mutex::new(None);
#[cfg(test)]
static TEST_FRAMES: Mutex<Vec<Snapshot>> = Mutex::new(Vec::new());
static MOTION_TIMER_ACTIVE: AtomicBool = AtomicBool::new(false);
static MOTION: OnceLock<Mutex<crate::clock_motion::Motion>> = OnceLock::new();
static GLASS_ACTIVE: AtomicBool = AtomicBool::new(false);
static MATERIAL_THEME: AtomicUsize = AtomicUsize::new(usize::MAX);
static SELECTED_PROVIDER: AtomicUsize = AtomicUsize::new(1);
static ALWAYS_ON_TOP: AtomicBool = AtomicBool::new(true);
static SIDEBAR_OPEN: AtomicBool = AtomicBool::new(false);
static SCROLL_GRAB: AtomicUsize = AtomicUsize::new(0);
static SIDE_SCROLL: AtomicUsize = AtomicUsize::new(0);
static MENU_OPEN: AtomicBool = AtomicBool::new(false);
static LIGHT_THEME: AtomicBool = AtomicBool::new(false);
static PRESSED: AtomicUsize = AtomicUsize::new(0);
static FOCUSED: AtomicUsize = AtomicUsize::new(0);
static HOVERED: AtomicUsize = AtomicUsize::new(0);
static DIAGNOSTIC_PAINT_COUNT: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub available: bool,
    pub remaining_percent: f64,
    pub refresh_minutes: u32,
    pub refreshing: bool,
    pub status: String,
    pub ahead_of_time: Option<bool>,
    pub starts_at: String,
    pub chinese: bool,
    pub provider_name: String,
    pub weekly: Option<String>,
    pub refresh_state: String,
    pub pace_reason: String,
    pub updated_at: Option<std::time::SystemTime>,
    pub pace_at: Option<std::time::SystemTime>,
    pub pace_cached: bool,
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
            WS_EX_APPWINDOW | WS_EX_LAYERED,
            PCWSTR::from_raw(class.as_ptr()),
            PCWSTR::from_raw(title.as_ptr()),
            WS_POPUP | WS_THICKFRAME | WS_MINIMIZEBOX | WS_MAXIMIZEBOX | WS_SYSMENU | WS_VISIBLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            scale(dpi, MAIN_WIDTH as i32),
            scale(dpi, VIEW_HEIGHT as i32),
            HWND::default(),
            HMENU::default(),
            instance,
            None,
        );
        if let Ok(hwnd) = hwnd {
            // Explicitly show the view even when the launcher used a hidden
            // startup window (the tray host itself is intentionally hidden).
            GLASS_ACTIVE.store(
                crate::clock_material::apply(hwnd, LIGHT_THEME.load(Ordering::Relaxed)),
                Ordering::Relaxed,
            );
            MATERIAL_THEME.store(
                LIGHT_THEME.load(Ordering::Relaxed) as usize,
                Ordering::Relaxed,
            );
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
                stop_motion_timer(hwnd);
            } else {
                let _ = SetTimer(hwnd, TIMER_CLOCK, 1000, None);
                let _ = InvalidateRect(hwnd, None, BOOL(0));
            }
            LRESULT(0)
        }
        WM_SHOWWINDOW => {
            if wparam.0 == 0 {
                let _ = KillTimer(hwnd, TIMER_CLOCK);
                stop_motion_timer(hwnd);
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
            stop_motion_timer(hwnd);
            let _ = ShowWindow(hwnd, SW_HIDE);
            LRESULT(0)
        }
        WM_NCCALCSIZE => LRESULT(0),
        WM_NCHITTEST => {
            let mut pt = POINT {
                x: lparam.0 as u16 as i16 as i32,
                y: (lparam.0 >> 16) as u16 as i16 as i32,
            };
            let _ = ScreenToClient(hwnd, &mut pt);
            let mut rect = RECT::default();
            let _ = GetClientRect(hwnd, &mut rect);
            let edge = scale(GetDpiForWindow(hwnd).max(96) as i32, 7);
            if !IsZoomed(hwnd).as_bool() {
                let left = pt.x < edge;
                let right = pt.x >= rect.right - edge;
                let top = pt.y < edge;
                let bottom = pt.y >= rect.bottom - edge;
                let hit = match (left, right, top, bottom) {
                    (true, _, true, _) => HTTOPLEFT,
                    (_, true, true, _) => HTTOPRIGHT,
                    (true, _, _, true) => HTBOTTOMLEFT,
                    (_, true, _, true) => HTBOTTOMRIGHT,
                    (true, _, _, _) => HTLEFT,
                    (_, true, _, _) => HTRIGHT,
                    (_, _, true, _) => HTTOP,
                    (_, _, _, true) => HTBOTTOM,
                    _ => HTCLIENT,
                };
                if hit != HTCLIENT {
                    return LRESULT(hit as isize);
                }
            }
            let layout = window_layout(hwnd);
            let (x, y) = layout.logical(pt.x, pt.y);
            if y < 44.0 && action_at(x, y, &layout) == 0 {
                LRESULT(HTCAPTION as isize)
            } else {
                LRESULT(HTCLIENT as isize)
            }
        }
        WM_MOUSEWHEEL => {
            let l = window_layout(hwnd);
            let mut pt = POINT {
                x: lparam.0 as u16 as i16 as i32,
                y: (lparam.0 >> 16) as u16 as i16 as i32,
            };
            let _ = ScreenToClient(hwnd, &mut pt);
            if l.open && f64::from(pt.x) / l.zoom >= l.main {
                let delta = (wparam.0 >> 16) as u16 as i16 as f64 / 120.0;
                SIDE_SCROLL.store(
                    (l.scroll - delta * 48.0).clamp(0.0, l.max_scroll()).round() as usize,
                    Ordering::Relaxed,
                );
                MENU_OPEN.store(false, Ordering::Relaxed);
                let _ = InvalidateRect(hwnd, None, BOOL(0));
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let mut track = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: hwnd,
                dwHoverTime: 0,
            };
            let _ = TrackMouseEvent(&mut track);
            let mut rect = RECT::default();
            let _ = GetClientRect(hwnd, &mut rect);
            let layout = window_layout(hwnd);
            let (x, y) = layout.logical(
                lparam.0 as u16 as i16 as i32,
                (lparam.0 >> 16) as u16 as i16 as i32,
            );
            if PRESSED.load(Ordering::Relaxed) == 30 {
                let fraction = ((y - 84.0 - SCROLL_GRAB.load(Ordering::Relaxed) as f64 / 100.0)
                    / (layout.height - 96.0 - layout.thumb().h).max(1.0))
                .clamp(0.0, 1.0);
                SIDE_SCROLL.store(
                    (fraction * layout.max_scroll()).round() as usize,
                    Ordering::Relaxed,
                );
                MENU_OPEN.store(false, Ordering::Relaxed);
                let _ = InvalidateRect(hwnd, None, BOOL(0));
                return LRESULT(0);
            }
            let hovered = action_at(x, y, &layout);
            if HOVERED.swap(hovered, Ordering::Relaxed) != hovered {
                let _ = InvalidateRect(hwnd, None, BOOL(0));
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let mut rect = RECT::default();
            let _ = GetClientRect(hwnd, &mut rect);
            let layout = window_layout(hwnd);
            let (x, y) = layout.logical(
                lparam.0 as u16 as i16 as i32,
                (lparam.0 >> 16) as u16 as i16 as i32,
            );
            let id = action_at(x, y, &layout);
            if MENU_OPEN.load(Ordering::Relaxed) && !(20..=23).contains(&id) && id != 6 {
                MENU_OPEN.store(false, Ordering::Relaxed);
                PRESSED.store(0, Ordering::Relaxed);
                let _ = InvalidateRect(hwnd, None, BOOL(0));
                return LRESULT(0);
            }
            if id == 30 {
                let thumb = layout.thumb();
                let grab = if y >= thumb.y && y <= thumb.y + thumb.h {
                    y - thumb.y
                } else {
                    thumb.h / 2.0
                };
                SCROLL_GRAB.store((grab * 100.0).round() as usize, Ordering::Relaxed);
                let fraction =
                    ((y - 84.0 - grab) / (layout.height - 96.0 - thumb.h).max(1.0)).clamp(0.0, 1.0);
                SIDE_SCROLL.store(
                    (fraction * layout.max_scroll()).round() as usize,
                    Ordering::Relaxed,
                );
            }
            PRESSED.store(id, Ordering::Relaxed);
            FOCUSED.store(0, Ordering::Relaxed);
            if id != 0 {
                let _ = SetCapture(hwnd);
            }
            let _ = InvalidateRect(hwnd, None, BOOL(0));
            LRESULT(0)
        }
        WM_CAPTURECHANGED => {
            PRESSED.store(0, Ordering::Relaxed);
            let _ = InvalidateRect(hwnd, None, BOOL(0));
            LRESULT(0)
        }
        WM_MOUSELEAVE | WM_NCMOUSEMOVE => {
            if HOVERED.swap(0, Ordering::Relaxed) != 0 {
                let _ = InvalidateRect(hwnd, None, BOOL(0));
            }
            LRESULT(0)
        }
        WM_KILLFOCUS => {
            FOCUSED.store(0, Ordering::Relaxed);
            let _ = InvalidateRect(hwnd, None, BOOL(0));
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let mut rect = RECT::default();
            let _ = GetClientRect(hwnd, &mut rect);
            let layout = window_layout(hwnd);
            let (x, y) = layout.logical(
                lparam.0 as u16 as i16 as i32,
                (lparam.0 >> 16) as u16 as i16 as i32,
            );
            let id = action_at(x, y, &layout);
            let pressed = PRESSED.swap(0, Ordering::Relaxed);
            let _ = ReleaseCapture();
            if id != 0 && id == pressed {
                activate_action(hwnd, id);
            }
            HOVERED.store(0, Ordering::Relaxed);
            let _ = InvalidateRect(hwnd, None, BOOL(0));
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            toggle_pin(hwnd);
            let _ = InvalidateRect(hwnd, None, BOOL(0));
            LRESULT(0)
        }
        WM_KEYDOWN => {
            match wparam.0 as u32 {
                0x1b if MENU_OPEN.load(Ordering::Relaxed) => {
                    MENU_OPEN.store(false, Ordering::Relaxed);
                    FOCUSED.store(6, Ordering::Relaxed);
                    let _ = InvalidateRect(hwnd, None, BOOL(0));
                }
                0x1b if SIDEBAR_OPEN.load(Ordering::Relaxed) => toggle_sidebar(hwnd),
                0x74 => crate::window::clock_refresh_command(None),
                0x54 => {
                    LIGHT_THEME.fetch_xor(true, Ordering::Relaxed);
                    let _ = InvalidateRect(hwnd, None, BOOL(0));
                }
                0x09 | 0x26 | 0x28 => {
                    if wparam.0 != 0x09 && !MENU_OPEN.load(Ordering::Relaxed) {
                        return LRESULT(0);
                    }
                    let actions = enabled_actions(SIDEBAR_OPEN.load(Ordering::Relaxed));
                    let old = FOCUSED.load(Ordering::Relaxed);
                    let index = actions.iter().position(|a| *a == old);
                    let back = wparam.0 == 0x26 || (wparam.0 == 0x09 && GetKeyState(0x10) < 0);
                    let next = match index {
                        Some(i) if back => (i + actions.len() - 1) % actions.len(),
                        Some(i) => (i + 1) % actions.len(),
                        None if back => actions.len() - 1,
                        None => 0,
                    };
                    FOCUSED.store(actions[next], Ordering::Relaxed);
                    focus_visible(hwnd, actions[next]);
                    let _ = InvalidateRect(hwnd, None, BOOL(0));
                }
                0x0d | 0x20 => {
                    activate_action(hwnd, FOCUSED.load(Ordering::Relaxed));
                }

                _ => {}
            }
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
            limits.ptMinTrackSize.x = scale(
                dpi,
                if SIDEBAR_OPEN.load(Ordering::Relaxed) {
                    648
                } else {
                    360
                },
            );
            limits.ptMinTrackSize.y = scale(dpi, 460);
            let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if GetMonitorInfoW(monitor, &mut info).as_bool() {
                limits.ptMaxPosition = POINT {
                    x: info.rcWork.left - info.rcMonitor.left,
                    y: info.rcWork.top - info.rcMonitor.top,
                };
                limits.ptMaxSize = POINT {
                    x: info.rcWork.right - info.rcWork.left,
                    y: info.rcWork.bottom - info.rcWork.top,
                };
            }

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
            stop_motion_timer(hwnd);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

// Curves are supersampled; text is drawn afterwards at native pixel resolution.
// Window resize changes available layout space, while font size follows OS DPI only.
fn colors() -> Palette {
    if LIGHT_THEME.load(Ordering::Relaxed) {
        clock_palette::LIGHT
    } else {
        clock_palette::DARK
    }
}

fn rgb(hex: u32) -> COLORREF {
    COLORREF(((hex & 255) << 16) | (hex & 0xff00) | ((hex >> 16) & 255))
}

#[derive(Clone, Copy, PartialEq)]
enum PaintLayer {
    Shapes,
    Text,
}
#[derive(Clone, Copy)]
struct Canvas {
    dc: HDC,
    glass: bool,
    mask: bool,
    opaque: bool,
    layer: PaintLayer,
    font_zoom: f64,
    zoom: f64,
    ox: f64,
    oy: f64,
}
impl Canvas {
    fn paint_color(&self, color: u32) -> COLORREF {
        rgb(crate::clock_material::paint_rgb(
            color,
            colors(),
            LIGHT_THEME.load(Ordering::Relaxed),
            self.glass,
            self.mask,
            self.opaque,
        ))
    }
    fn text_color(&self, color: u32) -> COLORREF {
        rgb(if self.mask { 0xffffff } else { color })
    }
    fn font_quality(&self) -> u32 {
        if self.glass {
            ANTIALIASED_QUALITY.0 as u32
        } else {
            CLEARTYPE_QUALITY.0 as u32
        }
    }
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
        if self.layer != PaintLayer::Shapes {
            return;
        }
        let brush = CreateSolidBrush(self.paint_color(fill));
        let pen = CreatePen(PS_SOLID, self.d(1.0), self.paint_color(border));
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
        if self.layer != PaintLayer::Shapes {
            return;
        }
        let pen = CreatePen(PS_SOLID, self.d(weight), self.paint_color(color));
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
        if self.layer != PaintLayer::Text || text.is_empty() {
            return;
        }
        let face = native_interop::wide_str(if text.is_ascii() {
            "Segoe UI"
        } else {
            "Microsoft YaHei UI"
        });
        let font = CreateFontW(
            -font_pixels(size, self.font_zoom),
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
            self.font_quality(),
            0,
            PCWSTR(face.as_ptr()),
        );
        let old = SelectObject(self.dc, font);
        let _ = SetBkMode(self.dc, TRANSPARENT);
        let _ = SetTextColor(self.dc, self.text_color(color));
        let mut chars: Vec<u16> = text.encode_utf16().collect();
        let mut rect = RECT {
            left: self.x(x),
            top: self.y(y),
            right: self.x(x + w),
            bottom: self.y(y) + font_pixels(size, self.font_zoom) * 2,
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
    unsafe fn paragraph(&self, text: &str, x: f64, y: f64, w: f64, h: f64, color: u32) {
        if self.layer != PaintLayer::Text || text.is_empty() {
            return;
        }
        let face = native_interop::wide_str("Microsoft YaHei UI");
        let font = CreateFontW(
            -font_pixels(12.0, self.font_zoom),
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            DEFAULT_CHARSET.0 as u32,
            OUT_TT_PRECIS.0 as u32,
            CLIP_DEFAULT_PRECIS.0 as u32,
            self.font_quality(),
            0,
            PCWSTR(face.as_ptr()),
        );
        let old = SelectObject(self.dc, font);
        let _ = SetBkMode(self.dc, TRANSPARENT);
        let _ = SetTextColor(self.dc, self.text_color(color));
        let mut chars: Vec<u16> = text.encode_utf16().collect();
        let mut rect = RECT {
            left: self.x(x),
            top: self.y(y),
            right: self.x(x + w),
            bottom: self.y(y + h),
        };
        let _ = DrawTextW(
            self.dc,
            &mut chars,
            &mut rect,
            DT_WORDBREAK | DT_NOPREFIX | DT_END_ELLIPSIS,
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
        if self.layer != PaintLayer::Shapes {
            return;
        }
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
        if self.layer != PaintLayer::Shapes {
            return;
        }
        let pen = CreatePen(PS_SOLID, self.d(weight), self.paint_color(color));
        let old = SelectObject(self.dc, pen);
        let _ = Polyline(self.dc, &pts);
        let _ = SelectObject(self.dc, old);
        let _ = DeleteObject(pen);
    }
    unsafe fn sector(&self, cx: f64, cy: f64, r: f64, start: f64, sweep: f64, color: u32) {
        if self.layer != PaintLayer::Shapes {
            return;
        }
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
        let brush = CreateSolidBrush(self.paint_color(color));
        let old = SelectObject(self.dc, brush);
        let op = SelectObject(self.dc, GetStockObject(NULL_PEN));
        let _ = Polygon(self.dc, &pts);
        let _ = SelectObject(self.dc, old);
        let _ = SelectObject(self.dc, op);
        let _ = DeleteObject(brush);
    }

    // Typeset along the circle at native resolution. Glyph size is set by DPI,
    // independently of the dial radius; no whole-window text resampling.
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
        if self.layer != PaintLayer::Text || text.is_empty() {
            return;
        }
        let bottom = angle.to_radians().sin() > 0.0;
        let pixels = font_pixels(size, self.font_zoom);
        let face = native_interop::wide_str("Microsoft YaHei UI");
        let make_font = |rotation: i32| {
            CreateFontW(
                -pixels,
                0,
                rotation,
                rotation,
                500,
                0,
                0,
                0,
                DEFAULT_CHARSET.0 as u32,
                OUT_TT_PRECIS.0 as u32,
                CLIP_DEFAULT_PRECIS.0 as u32,
                self.font_quality(),
                0,
                PCWSTR(face.as_ptr()),
            )
        };
        let measure = make_font(0);
        let old = SelectObject(self.dc, measure);
        let chars: Vec<Vec<u16>> = text
            .chars()
            .map(|ch| ch.to_string().encode_utf16().collect())
            .collect();
        let advances: Vec<f64> = chars
            .iter()
            .map(|glyph| {
                let mut extent = SIZE::default();
                let _ = GetTextExtentPoint32W(self.dc, glyph, &mut extent);
                f64::from(extent.cx) / self.zoom
            })
            .collect();
        let _ = SelectObject(self.dc, old);
        let _ = DeleteObject(measure);
        let total: f64 = advances.iter().sum();
        let mut offset = -total / 2.0;
        for (glyph, advance) in chars.iter().zip(advances.iter()) {
            let a = angle
                + if bottom { -1.0 } else { 1.0 } * ((offset + advance / 2.0) / r).to_degrees();
            let (x, y) = point(cx, cy, r, a);
            let rotation = if bottom { 90.0 - a } else { -90.0 - a };
            let font = make_font((rotation * 10.0).round() as i32);
            let old = SelectObject(self.dc, font);
            let previous = SetTextAlign(self.dc, TA_CENTER | TA_BASELINE);
            let _ = SetBkMode(self.dc, TRANSPARENT);
            let _ = SetTextColor(self.dc, self.text_color(color));
            let _ = TextOutW(self.dc, self.x(x), self.y(y), glyph);
            let _ = SetTextAlign(self.dc, TEXT_ALIGN_OPTIONS(previous));
            let _ = SelectObject(self.dc, old);
            let _ = DeleteObject(font);
            offset += advance;
        }
    }
    // Match the individual glyph advances used by curved_text, in dial units.
    unsafe fn curved_text_width(&self, text: &str, size: f64) -> f64 {
        let face = native_interop::wide_str("Microsoft YaHei UI");
        let font = CreateFontW(
            -font_pixels(size, self.font_zoom),
            0,
            0,
            0,
            500,
            0,
            0,
            0,
            DEFAULT_CHARSET.0 as u32,
            OUT_TT_PRECIS.0 as u32,
            CLIP_DEFAULT_PRECIS.0 as u32,
            self.font_quality(),
            0,
            PCWSTR(face.as_ptr()),
        );
        let old = SelectObject(self.dc, font);
        let width: i32 = text
            .chars()
            .map(|ch| {
                let glyph: Vec<u16> = ch.to_string().encode_utf16().collect();
                let mut extent = SIZE::default();
                let _ = GetTextExtentPoint32W(self.dc, &glyph, &mut extent);
                extent.cx
            })
            .sum();
        let _ = SelectObject(self.dc, old);
        let _ = DeleteObject(font);
        f64::from(width) / self.zoom
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
        let u = self.font_zoom / self.zoom;
        let radius = tag_radius(self);
        // Keep the capsule and its two text rows readable at minimum size.
        let width = self
            .curved_text_width(label, 13.0)
            .max(self.curved_text_width(value, 15.0));
        let half = ((34.0 * u).max(width / 2.0 - 6.0 * u) / radius).to_degrees();
        self.arc(
            cx,
            cy,
            radius,
            angle - half,
            half * 2.0,
            46.0 * u,
            blend(color, colors().line, 0.3),
        );
        self.arc(
            cx,
            cy,
            radius,
            angle - half,
            half * 2.0,
            44.0 * u,
            if self.glass {
                surface
            } else {
                blend(surface, colors().field, 0.4)
            },
        );
        let bottom = angle.to_radians().sin() > 0.0;
        self.curved_text(
            label,
            cx,
            cy,
            radius + if bottom { -5.0 * u } else { 4.0 * u },
            angle,
            13.0,
            colors().muted,
        );
        self.curved_text(
            value,
            cx,
            cy,
            radius + if bottom { 13.0 * u } else { -13.0 * u },
            angle,
            15.0,
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
    let now = s.pace_at.unwrap_or(now);
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

const MAIN_WIDTH: f64 = 420.0;
const SIDE_WIDTH: f64 = 288.0;
const VIEW_HEIGHT: f64 = 540.0;
const CONTENT_BOTTOM: f64 = 674.0;
fn font_pixels(size: f64, dpi_scale: f64) -> i32 {
    (size.max(13.0) * dpi_scale).round().max(1.0) as i32
}
fn tag_radius(c: &Canvas) -> f64 {
    // Keep at least 8 DIP between the tag inner edge and the outer pace ring.
    238.0f64.max(197.0 + 30.0 * self_font_unit(c))
}
fn self_font_unit(c: &Canvas) -> f64 {
    c.font_zoom / c.zoom
}
#[derive(Clone, Copy, Debug)]
struct Bounds {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}
impl Bounds {
    fn contains(self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}
struct Layout {
    width: f64,
    height: f64,
    main: f64,
    zoom: f64,
    open: bool,
    scroll: f64,
}
impl Layout {
    fn new(w: i32, h: i32, open: bool, dpi: u32, scroll: usize) -> Self {
        let zoom = f64::from(dpi.max(96)) / 96.0;
        let width = f64::from(w) / zoom;
        let height = f64::from(h) / zoom;
        Self {
            width,
            height,
            main: width - if open { SIDE_WIDTH } else { 0.0 },
            zoom,
            open,
            scroll: (scroll as f64).min((CONTENT_BOTTOM - height + 12.0).max(0.0)),
        }
    }
    fn logical(&self, x: i32, y: i32) -> (f64, f64) {
        (f64::from(x) / self.zoom, f64::from(y) / self.zoom)
    }
    fn max_scroll(&self) -> f64 {
        (CONTENT_BOTTOM - self.height + 12.0).max(0.0)
    }
    fn thumb(&self) -> Bounds {
        let track = self.height - 96.0;
        let h = (track * track / (track + self.max_scroll())).max(28.0);
        Bounds {
            x: self.width - 6.0,
            y: 84.0 + (track - h) * self.scroll / self.max_scroll().max(1.0),
            w: 3.0,
            h,
        }
    }
    fn menu(&self) -> Bounds {
        let below = 493.0 - self.scroll + 36.0;
        let y = if below + 136.0 <= self.height - 8.0 {
            below
        } else {
            493.0 - self.scroll - 136.0
        };
        Bounds {
            x: self.main + 136.0,
            y: y.clamp(84.0, (self.height - 144.0).max(84.0)),
            w: 136.0,
            h: 136.0,
        }
    }
    fn control(&self, id: usize) -> Option<Bounds> {
        let rect = match id {
            1 => Bounds {
                x: 10.0,
                y: 8.0,
                w: 28.0,
                h: 28.0,
            },
            9 | 2 | 3 | 4 | 5 => {
                let offset = match id {
                    9 => 180.0,
                    2 => 144.0,
                    3 => 108.0,
                    4 => 72.0,
                    _ => 36.0,
                };
                Bounds {
                    x: self.width - offset,
                    y: 8.0,
                    w: 28.0,
                    h: 28.0,
                }
            }
            8 => Bounds {
                x: 12.0,
                y: 50.0,
                w: 145.0,
                h: 30.0,
            },
            6 if self.open => Bounds {
                x: self.main + 136.0,
                y: 493.0 - self.scroll,
                w: 136.0,
                h: 32.0,
            },
            7 if self.open => Bounds {
                x: self.main + 16.0,
                y: 537.0 - self.scroll,
                w: 256.0,
                h: 34.0,
            },
            10..=12 if self.open => Bounds {
                x: self.main + 16.0 + (id - 10) as f64 * 87.0,
                y: 88.0 - self.scroll,
                w: 82.0,
                h: 32.0,
            },
            20..=23 if self.open && MENU_OPEN.load(Ordering::Relaxed) => {
                let m = self.menu();
                Bounds {
                    x: m.x + 3.0,
                    y: m.y + 3.0 + (id - 20) as f64 * 33.0,
                    w: 130.0,
                    h: 32.0,
                }
            }
            _ => return None,
        };
        Some(rect)
    }
}
unsafe fn window_layout(hwnd: HWND) -> Layout {
    let mut rect = RECT::default();
    let _ = GetClientRect(hwnd, &mut rect);
    Layout::new(
        rect.right,
        rect.bottom,
        SIDEBAR_OPEN.load(Ordering::Relaxed),
        GetDpiForWindow(hwnd),
        SIDE_SCROLL.load(Ordering::Relaxed),
    )
}
fn action_at(x: f64, y: f64, l: &Layout) -> usize {
    if MENU_OPEN.load(Ordering::Relaxed) && l.open {
        for id in 20..=23 {
            if l.control(id).is_some_and(|r| r.contains(x, y)) {
                return id;
            }
        }
        if l.menu().contains(x, y) {
            return 0;
        }
    }
    for id in [1, 9, 2, 3, 4, 5, 8] {
        if l.control(id).is_some_and(|r| r.contains(x, y)) {
            return id;
        }
    }
    if l.open && x >= l.main && y >= 84.0 && y < l.height - 8.0 {
        if x >= l.width - 9.0 && l.max_scroll() > 0.0 {
            return 30;
        }
        for id in [6, 7, 10, 11, 12] {
            if l.control(id).is_some_and(|r| r.contains(x, y)) {
                return id;
            }
        }
    }
    0
}
unsafe fn focus_visible(hwnd: HWND, id: usize) {
    let l = window_layout(hwnd);
    if matches!(id, 6 | 7 | 10..=12) {
        if let Some(r) = l.control(id) {
            let next = if r.y < 84.0 {
                l.scroll + r.y - 84.0
            } else if r.y + r.h > l.height - 12.0 {
                l.scroll + r.y + r.h - l.height + 12.0
            } else {
                l.scroll
            };
            SIDE_SCROLL.store(
                next.clamp(0.0, l.max_scroll()).round() as usize,
                Ordering::Relaxed,
            );
        }
    }
}
unsafe fn activate_action(hwnd: HWND, id: usize) {
    if (20..=23).contains(&id) && !MENU_OPEN.load(Ordering::Relaxed) {
        return;
    }
    if matches!(id,6|7|10..=12|20..=23) && !SIDEBAR_OPEN.load(Ordering::Relaxed) {
        return;
    }
    match id {
        1 => toggle_sidebar(hwnd),
        2 => toggle_pin(hwnd),
        3 => {
            let _ = ShowWindow(hwnd, SW_MINIMIZE);
        }
        4 => {
            let _ = ShowWindow(
                hwnd,
                if IsZoomed(hwnd).as_bool() {
                    SW_RESTORE
                } else {
                    SW_MAXIMIZE
                },
            );
        }
        5 => {
            let _ = SendMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
        }
        6 => {
            MENU_OPEN.fetch_xor(true, Ordering::Relaxed);
        }
        7 if !crate::window::clock_snapshot(selected_provider()).refreshing => {
            crate::window::clock_refresh_command(None)
        }
        id @ 20..=23 => {
            crate::window::clock_refresh_command(Some([1, 5, 15, 60][id - 20]));
            MENU_OPEN.store(false, Ordering::Relaxed);
            FOCUSED.store(6, Ordering::Relaxed);
        }
        8 => next_provider(),
        9 => {
            LIGHT_THEME.fetch_xor(true, Ordering::Relaxed);
        }
        id @ 10..=12 => {
            let provider = id - 10;
            if crate::window::clock_enabled_providers()[provider] {
                SELECTED_PROVIDER.store(provider, Ordering::Relaxed);
            }
        }
        _ => {}
    }
    let _ = InvalidateRect(hwnd, None, BOOL(0));
}
fn enabled_actions(open: bool) -> Vec<usize> {
    if MENU_OPEN.load(Ordering::Relaxed) && open {
        return vec![20, 21, 22, 23];
    }
    let mut actions = vec![1, 9, 2, 3, 4, 5, 8];
    if open {
        for (i, enabled) in crate::window::clock_enabled_providers().iter().enumerate() {
            if *enabled {
                actions.push(10 + i);
            }
        }
        actions.push(6);
        if !crate::window::clock_snapshot(selected_provider()).refreshing {
            actions.push(7);
        }
    }
    actions
}

unsafe fn toggle_sidebar(hwnd: HWND) {
    let open = SIDEBAR_OPEN.load(Ordering::Relaxed);
    let l = window_layout(hwnd);
    SIDEBAR_OPEN.store(!open, Ordering::Relaxed);
    MENU_OPEN.store(false, Ordering::Relaxed);
    SIDE_SCROLL.store(0, Ordering::Relaxed);
    if FOCUSED.load(Ordering::Relaxed) != 0 {
        FOCUSED.store(1, Ordering::Relaxed);
    }
    let delta = (SIDE_WIDTH * l.zoom).round() as i32 * if open { -1 } else { 1 };
    if IsZoomed(hwnd).as_bool() {
        let mut p = WINDOWPLACEMENT {
            length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
            ..Default::default()
        };
        if GetWindowPlacement(hwnd, &mut p).is_ok() {
            p.rcNormalPosition.right += delta;
            let _ = SetWindowPlacement(hwnd, &p);
        }
    } else {
        let mut r = RECT::default();
        let _ = GetWindowRect(hwnd, &mut r);
        let width = r.right - r.left + delta;
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let mut x = r.left;
        if GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut info).as_bool() {
            x = x.min(info.rcWork.right - width).max(info.rcWork.left);
        }
        let _ = SetWindowPos(
            hwnd,
            HWND::default(),
            x,
            r.top,
            width,
            r.bottom - r.top,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
    let _ = InvalidateRect(hwnd, None, BOOL(0));
}
unsafe fn toggle_pin(hwnd: HWND) {
    let top = !ALWAYS_ON_TOP.fetch_xor(true, Ordering::Relaxed);
    let _ = SetWindowPos(
        hwnd,
        if top { HWND_TOPMOST } else { HWND_NOTOPMOST },
        0,
        0,
        0,
        0,
        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
    );
}
unsafe fn draw_dial(c: &Canvas, s: &Snapshot, now: std::time::SystemTime) {
    let tr = |a, b| if s.chinese { a } else { b };
    let (cx, cy) = (328.0, 407.0);
    let r = 168.0;
    c.panel(
        cx - 179.0,
        cy - 179.0,
        358.0,
        358.0,
        179.0,
        colors().face,
        colors().line,
    );
    let geometry = window_geometry(s, now);
    let mut pace_color = colors().time;
    let mut diff = None;
    if let Some((start, elapsed, sweep, delta)) = geometry {
        let a = clock_angle(start).unwrap_or(-90.0);
        let quota = a + (1.0 - s.remaining_percent / 100.0) * sweep;
        let time = a + elapsed * sweep;
        c.arc(cx, cy, 189.0, a, sweep, 10.0, colors().track);
        c.arc(cx, cy, 189.0, a, sweep, 4.0, colors().track);
        if s.ahead_of_time.is_some() {
            pace_color = if delta > 0.0 {
                colors().safe
            } else if delta < 0.0 {
                colors().risk
            } else {
                colors().time
            };
            diff = Some(delta);
            c.sector(
                cx,
                cy,
                r,
                quota.min(time),
                (quota - time).abs(),
                if delta >= 0.0 {
                    colors().safe_surface
                } else {
                    colors().risk_surface
                },
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
            (a, colors().time, 201.0, 215.0),
            (a + sweep, colors().warning, 201.0, 215.0),
            (time, colors().time, 181.0, 203.0),
            (quota, colors().text, 181.0, 213.0),
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
            angle
                + direction
                    * (50.0f64.max((132.0 * self_font_unit(c) / 238.0).to_degrees()) - gap).max(0.0)
        };
        let start_label = separate(a, -1.0);
        let end_label = separate(a + sweep, 1.0);
        for (exact, label, color) in [
            (a, start_label, colors().time),
            (a + sweep, end_label, colors().warning),
        ] {
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
            colors().time,
            colors().field,
        );
        c.tag(
            cx,
            cy,
            end_label,
            tr("额度重置", "QUOTA RESET"),
            &s.reset_at.map(time_label).unwrap_or_default(),
            colors().warning,
            colors().field,
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
                    colors().safe_surface
                } else {
                    colors().risk_surface
                }
            } else {
                colors().field
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
                c.arc(
                    cx,
                    cy,
                    142.0,
                    a,
                    (b - a).rem_euclid(360.0),
                    2.5,
                    colors().time,
                );
                c.line(
                    point(cx, cy, 135.0, b),
                    point(cx, cy, 152.0, b),
                    3.0,
                    colors().time,
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
            if major { colors().text } else { colors().tick },
        );
    }
    // Enlarge dial numerals with the dial while retaining the small-window floor.
    // Recreate the font at its final size rather than stretching rasterized text.
    let numeral_growth = ((c.zoom / c.font_zoom) / 0.68).clamp(1.0, 2.2);
    let numerals = Canvas {
        font_zoom: c.font_zoom * numeral_growth,
        ..*c
    };
    for hour in 1..=12 {
        let (x, y) = point(cx, cy, 128.0, -90.0 + f64::from(hour) * 30.0);
        numerals.text(
            &hour.to_string(),
            x - 21.0,
            y - 8.5 * numeral_growth * self_font_unit(c),
            42.0,
            17.0,
            colors().text,
            true,
            true,
        );
    }
    if let Some(t) = native_interop::system_time_to_local(now) {
        let sec = f64::from(t.wSecond);
        let min = f64::from(t.wMinute) + sec / 60.0;
        let hour = f64::from(t.wHour % 12) + min / 60.0;
        for (a, len, tail, weight, color) in [
            (hour * 30.0 - 90.0, 77.0, 15.0, 7.2, colors().text),
            (min * 6.0 - 90.0, 111.0, 18.0, 7.2, colors().text),
            (sec * 6.0 - 90.0, 148.0, 22.0, 2.0, colors().time),
        ] {
            c.line(
                point(cx, cy, tail, a + 180.0),
                point(cx, cy, len, a),
                weight,
                color,
            );
        }
    }
    c.panel(
        cx - 7.0,
        cy - 7.0,
        14.0,
        14.0,
        7.0,
        colors().face,
        colors().warning,
    );
    c.panel(
        cx - 3.0,
        cy - 3.0,
        6.0,
        6.0,
        3.0,
        colors().text,
        colors().text,
    );
}

unsafe fn toolbar_icon(c: &Canvas, id: usize, r: Bounds, active: bool) {
    let p = colors();
    let hot = HOVERED.load(Ordering::Relaxed) == id;
    let fill = if hot && id == 5 {
        0xb63131
    } else if PRESSED.load(Ordering::Relaxed) == id {
        p.selected
    } else if hot {
        p.hover
    } else if active {
        p.field
    } else {
        p.window
    };
    c.panel(
        r.x,
        r.y,
        r.w,
        r.h,
        6.0,
        fill,
        if FOCUSED.load(Ordering::Relaxed) == id {
            p.accent
        } else {
            fill
        },
    );
    let color = if hot && id == 5 {
        0xffffff
    } else if active {
        p.text
    } else {
        p.muted
    };
    let (x, y) = (r.x + 14.0, r.y + 14.0);
    match id {
        1 => {
            c.panel(x - 7.0, y - 6.0, 14.0, 12.0, 1.0, fill, color);
            c.line((x - 1.0, y - 5.0), (x - 1.0, y + 5.0), 1.0, color);
        }
        9 => {
            c.arc(x, y, 5.0, 0.0, 360.0, 1.0, color);
            c.line((x, y - 5.0), (x, y + 5.0), 1.0, color);
        }
        2 => {
            c.line((x - 4.0, y - 6.0), (x + 4.0, y - 6.0), 1.0, color);
            c.line((x - 3.0, y - 5.0), (x - 3.0, y), 1.0, color);
            c.line((x + 3.0, y - 5.0), (x + 3.0, y), 1.0, color);
            c.line((x - 5.0, y + 1.0), (x + 5.0, y + 1.0), 1.0, color);
            c.line((x, y + 1.0), (x, y + 7.0), 1.0, color);
        }
        3 => c.line((x - 5.0, y), (x + 5.0, y), 1.0, color),
        4 => c.panel(x - 5.0, y - 5.0, 10.0, 10.0, 1.0, fill, color),
        5 => {
            c.line((x - 4.0, y - 4.0), (x + 4.0, y + 4.0), 1.0, color);
            c.line((x + 4.0, y - 4.0), (x - 4.0, y + 4.0), 1.0, color);
        }
        _ => {}
    }
}
unsafe fn control(
    c: &Canvas,
    id: usize,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    label: &str,
    selected: bool,
    disabled: bool,
    primary: bool,
) {
    let p = colors();
    let hot = HOVERED.load(Ordering::Relaxed) == id && !disabled;
    let down = PRESSED.load(Ordering::Relaxed) == id && !disabled;
    let bg = if disabled {
        p.sidebar
    } else if down {
        p.selected
    } else if primary {
        p.accent
    } else if hot {
        p.hover
    } else if selected {
        p.selected
    } else {
        p.field
    };
    let border = if FOCUSED.load(Ordering::Relaxed) == id {
        p.accent
    } else if selected {
        blend(p.accent, p.line, 0.5)
    } else {
        p.line
    };
    let text = if disabled {
        p.disabled
    } else if primary && !down {
        if LIGHT_THEME.load(Ordering::Relaxed) {
            0xffffff
        } else {
            p.window
        }
    } else {
        p.text
    };
    c.panel(x, y, w, h, 7.0, bg, border);
    c.text(
        label,
        x + 6.0,
        y + (h - 18.0) / 2.0,
        w - 12.0,
        12.0,
        text,
        selected || primary,
        true,
    );
}

unsafe fn control_hint(c: &Canvas, s: &Snapshot, l: &Layout) {
    let foreground = Canvas { opaque: true, ..*c };
    let c = &foreground;
    let hovered = HOVERED.load(Ordering::Relaxed);
    let id = if hovered == 0 {
        FOCUSED.load(Ordering::Relaxed)
    } else {
        hovered
    };
    let tr = |a, b| if s.chinese { a } else { b };
    let label = match id {
        1 => tr("展开 / 收起详情 · Esc", "Toggle details · Esc"),
        2 => tr("切换窗口置顶", "Toggle pin"),
        3 => tr("最小化", "Minimize"),
        4 => tr("最大化 / 还原", "Maximize / restore"),
        5 => tr("隐藏钟表，继续后台监控", "Hide clock; keep monitoring"),
        8 => tr("切换已启用服务", "Cycle enabled services"),
        9 => tr("切换深浅主题 · T", "Switch theme · T"),
        _ => return,
    };
    let x = if id == 1 || id == 8 {
        10.0
    } else {
        (l.width - 242.0).max(10.0)
    };
    let y = if id == 8 { 84.0 } else { 44.0 };
    c.panel(x, y, 232.0, 30.0, 6.0, colors().field, colors().strong);
    c.text(
        label,
        x + 6.0,
        y + 6.0,
        220.0,
        12.0,
        colors().text,
        false,
        true,
    );
}
unsafe fn scene(c: &Canvas, s: &Snapshot, now: std::time::SystemTime, l: &Layout) {
    let p = colors();
    let tr = |a, b| if s.chinese { a } else { b };
    c.panel(0.0, 0.0, l.width, l.height, 8.0, p.window, p.window);
    c.line((1.0, 43.0), (l.width - 1.0, 43.0), 1.0, p.line);
    for id in [1, 9, 2, 3, 4, 5] {
        toolbar_icon(
            c,
            id,
            l.control(id).unwrap(),
            (id == 1 && l.open) || (id == 2 && ALWAYS_ON_TOP.load(Ordering::Relaxed)),
        );
    }
    c.text("PACE", 46.0, 14.0, 82.0, 13.0, p.text, true, false);
    let r = l.control(8).unwrap();
    control(
        c,
        8,
        r.x,
        r.y,
        r.w,
        r.h,
        &format!("{}  ↻", s.provider_name),
        false,
        false,
        false,
    );
    let duration = s
        .window_seconds
        .filter(|v| *v > 0)
        .map(|v| format!("{}H WINDOW", v as f64 / 3600.0))
        .unwrap_or_else(|| "SESSION".into());
    c.text(
        &duration,
        l.main - 111.0,
        58.0,
        98.0,
        12.0,
        p.muted,
        false,
        true,
    );
    let dial_scale = ((l.main - 30.0) / 548.0)
        .min((l.height - 168.0) / 548.0)
        .max(0.3);
    let cx = l.main / 2.0;
    let cy = (86.0 + l.height - 86.0) / 2.0;
    let dial = Canvas {
        dc: c.dc,
        glass: c.glass,
        mask: c.mask,
        opaque: c.opaque,
        layer: c.layer,
        font_zoom: c.font_zoom,
        zoom: c.zoom * dial_scale,
        ox: (c.ox + cx) / dial_scale - 328.0,
        oy: (c.oy + cy) / dial_scale - 407.0,
    };
    draw_dial(&dial, s, now);
    if let Some(t) = native_interop::system_time_to_local(now) {
        c.text(
            &format!("{:02}:{:02}:{:02}", t.wHour, t.wMinute, t.wSecond),
            12.0,
            l.height - 66.0,
            l.main - 24.0,
            28.0,
            p.text,
            false,
            true,
        );
    }
    let geometry = window_geometry(s, now);
    let delta = geometry.filter(|_| s.ahead_of_time.is_some()).map(|v| v.3);
    let color = match delta {
        Some(v) if v > 1e-7 => p.safe,
        Some(v) if v < -1e-7 => p.risk,
        _ => p.muted,
    };
    let summary = if s.pace_cached {
        tr("上次节奏 · 等待更新", "Last pace · awaiting update").into()
    } else {
        match delta {
            Some(v) if v > 1e-7 => format!("{} · +{:.1} pp", tr("用得慢", "Slower use"), v),
            Some(v) if v < -1e-7 => format!("{} · {:.1} pp", tr("用得快", "Faster use"), v),
            Some(_) => tr("额度与时间同步", "Quota matches time").into(),
            None => {
                if s.available {
                    tr("等待有效时间窗口", "Awaiting valid window").into()
                } else {
                    tr("等待额度数据", "Awaiting quota data").into()
                }
            }
        }
    };
    c.text(
        &summary,
        12.0,
        l.height - 26.0,
        l.main - 24.0,
        12.0,
        color,
        false,
        true,
    );
    if !l.open {
        control_hint(c, s, l);
        return;
    }
    let x = l.main;
    c.panel(
        x,
        44.0,
        SIDE_WIDTH - 1.0,
        l.height - 45.0,
        0.0,
        p.sidebar,
        p.sidebar,
    );
    c.line((x, 44.0), (x, l.height - 1.0), 1.0, p.line);
    c.text(
        tr("额度与设置", "Quota & settings"),
        x + 16.0,
        57.0,
        256.0,
        16.0,
        p.text,
        true,
        false,
    );
    let saved = SaveDC(c.dc);
    let _ = IntersectClipRect(
        c.dc,
        c.x(x + 1.0),
        c.y(84.0),
        c.x(l.width - 1.0),
        c.y(l.height - 8.0),
    );
    let sc = Canvas {
        dc: c.dc,
        glass: c.glass,
        mask: c.mask,
        opaque: c.opaque,
        layer: c.layer,
        font_zoom: c.font_zoom,
        zoom: c.zoom,
        ox: c.ox,
        oy: c.oy - l.scroll,
    };
    let enabled = crate::window::clock_enabled_providers();
    for (i, name) in ["Claude", "Codex", "Antigravity"].iter().enumerate() {
        let selected = s.provider_name == ["Claude Code", "Codex", "Antigravity"][i];
        control(
            &sc,
            10 + i,
            x + 16.0 + i as f64 * 87.0,
            88.0,
            82.0,
            32.0,
            name,
            selected,
            !enabled[i] && !selected,
            false,
        );
    }
    sc.panel(x + 16.0, 136.0, 256.0, 174.0, 8.0, p.field, p.line);
    sc.text(
        tr("消耗节奏", "QUOTA PACE"),
        x + 28.0,
        148.0,
        232.0,
        12.0,
        p.muted,
        false,
        false,
    );
    sc.text(
        &delta
            .map(|v| format!("{:+.1} pp", v))
            .unwrap_or_else(|| "-- pp".into()),
        x + 28.0,
        171.0,
        232.0,
        27.0,
        color,
        false,
        false,
    );
    let pace = match delta {
        Some(v) if v > 1e-7 => tr("额度消耗慢于时间推进", "Quota use is behind time"),
        Some(v) if v < -1e-7 => tr("额度消耗快于时间推进", "Quota use is ahead of time"),
        Some(_) => tr("额度与时间同步", "Quota matches time"),
        None => s.pace_reason.as_str(),
    };
    sc.text(pace, x + 28.0, 211.0, 232.0, 12.0, p.muted, false, false);
    for (i, label) in [tr("时间已走", "Time elapsed"), tr("额度已用", "Quota used")]
        .iter()
        .enumerate()
    {
        let y = 240.0 + i as f64 * 33.0;
        let value = if i == 0 {
            geometry.map(|v| v.1 * 100.0)
        } else {
            if s.available {
                Some(100.0 - s.remaining_percent)
            } else {
                None
            }
        };
        sc.text(label, x + 28.0, y, 130.0, 12.0, p.muted, false, false);
        sc.text(
            &value
                .map(|v| format!("{:.1}%", v))
                .unwrap_or_else(|| "--".into()),
            x + 182.0,
            y,
            78.0,
            12.0,
            p.text,
            false,
            true,
        );
        sc.panel(x + 28.0, y + 22.0, 232.0, 4.0, 2.0, p.line, p.line);
        if let Some(v) = value {
            if v > 0.0 {
                let fill = if i == 0 { p.time } else { color };
                sc.panel(
                    x + 28.0,
                    y + 22.0,
                    232.0 * v.clamp(0.0, 100.0) / 100.0,
                    4.0,
                    2.0,
                    fill,
                    fill,
                );
            }
        }
    }
    sc.text(
        tr("剩余额度", "Remaining"),
        x + 16.0,
        328.0,
        120.0,
        12.0,
        p.muted,
        false,
        false,
    );
    sc.text(
        tr("距离重置", "Resets in"),
        x + 152.0,
        328.0,
        120.0,
        12.0,
        p.muted,
        false,
        false,
    );
    sc.text(
        &if s.available {
            format!("{:.0}%", s.remaining_percent)
        } else {
            "--".into()
        },
        x + 16.0,
        352.0,
        120.0,
        25.0,
        p.text,
        false,
        false,
    );
    let countdown = s
        .reset_at
        .and_then(|t| t.duration_since(now).ok())
        .map(|d| {
            format!(
                "{:02}:{:02}:{:02}",
                d.as_secs() / 3600,
                d.as_secs() % 3600 / 60,
                d.as_secs() % 60
            )
        })
        .unwrap_or_else(|| "--:--:--".into());
    sc.text(
        &countdown,
        x + 152.0,
        358.0,
        120.0,
        20.0,
        p.text,
        false,
        false,
    );
    sc.line((x + 16.0, 390.0), (x + 272.0, 390.0), 1.0, p.line);
    sc.text(
        tr("推导起点 — 额度重置", "Derived start — quota reset"),
        x + 16.0,
        403.0,
        256.0,
        12.0,
        p.muted,
        false,
        false,
    );
    sc.text(
        &format!(
            "{} — {}",
            s.starts_at,
            s.reset_at.map(time_label).unwrap_or_else(|| "--:--".into())
        ),
        x + 16.0,
        425.0,
        256.0,
        15.0,
        p.text,
        false,
        false,
    );
    sc.paragraph(
        s.weekly.as_deref().unwrap_or(""),
        x + 16.0,
        454.0,
        256.0,
        32.0,
        p.muted,
    );
    sc.text(
        tr("自动刷新", "Auto refresh"),
        x + 16.0,
        501.0,
        112.0,
        12.0,
        p.muted,
        false,
        false,
    );
    control(
        &sc,
        6,
        x + 136.0,
        493.0,
        136.0,
        32.0,
        &format!(
            "{} {} {}  ▾",
            tr("每", "Every"),
            s.refresh_minutes.max(1),
            tr("分钟", "min")
        ),
        false,
        false,
        false,
    );
    control(
        &sc,
        7,
        x + 16.0,
        537.0,
        256.0,
        34.0,
        if s.refreshing {
            tr("正在刷新…", "Refreshing…")
        } else {
            tr("立即刷新", "Refresh now")
        },
        false,
        s.refreshing,
        true,
    );
    sc.paragraph(&s.refresh_state, x + 16.0, 585.0, 256.0, 36.0, p.muted);
    sc.paragraph(&s.status, x + 16.0, 627.0, 256.0, 42.0, p.warning);
    let _ = RestoreDC(c.dc, saved);
    if l.max_scroll() > 0.0 {
        let r = l.thumb();
        c.panel(r.x, r.y, r.w, r.h, 1.5, p.strong, p.strong);
    }
    if MENU_OPEN.load(Ordering::Relaxed) {
        let overlay = Canvas { opaque: true, ..*c };
        let c = &overlay;
        let m = l.menu();
        c.panel(m.x, m.y, m.w, m.h, 6.0, p.field, p.strong);
        for (i, minutes) in [1, 5, 15, 60].iter().enumerate() {
            let r = l.control(20 + i).unwrap();
            control(
                c,
                20 + i,
                r.x,
                r.y,
                r.w,
                r.h,
                &format!("{} {} {}", tr("每", "Every"), minutes, tr("分钟", "min")),
                s.refresh_minutes == *minutes,
                false,
                false,
            );
        }
    }
    control_hint(c, s, l);
}

struct Surface {
    dc: HDC,
    bitmap: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u8,
    w: i32,
    h: i32,
}
impl Surface {
    unsafe fn new(target: HDC, w: i32, h: i32) -> Option<Self> {
        let dc = CreateCompatibleDC(target);
        if dc.is_invalid() {
            return None;
        }
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let bitmap =
            match CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, HANDLE::default(), 0) {
                Ok(b) => b,
                Err(_) => {
                    let _ = DeleteDC(dc);
                    return None;
                }
            };
        let old = SelectObject(dc, bitmap);
        Some(Self {
            dc,
            bitmap,
            old,
            bits: bits as *mut u8,
            w,
            h,
        })
    }
    unsafe fn pixels(&self) -> &mut [u8] {
        std::slice::from_raw_parts_mut(self.bits, (self.w * self.h * 4) as usize)
    }
}
impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            let _ = SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bitmap);
            let _ = DeleteDC(self.dc);
        }
    }
}
unsafe fn draw_surface(
    target: HDC,
    l: &Layout,
    s: &Snapshot,
    now: std::time::SystemTime,
    glass: bool,
    mask: bool,
) -> Option<Surface> {
    let w = (l.width * l.zoom).round() as i32;
    let h = (l.height * l.zoom).round() as i32;
    // Supersample geometry only; text remains at native DPI resolution.
    let aa = 4;
    let shapes = Surface::new(target, w * aa, h * aa)?;
    let frame = Surface::new(target, w, h)?;
    let canvas = Canvas {
        dc: shapes.dc,
        glass,
        mask,
        opaque: false,
        layer: PaintLayer::Shapes,
        font_zoom: l.zoom * f64::from(aa),
        zoom: l.zoom * f64::from(aa),
        ox: 0.0,
        oy: 0.0,
    };
    let bg = CreateSolidBrush(canvas.paint_color(colors().window));
    let _ = FillRect(
        shapes.dc,
        &RECT {
            left: 0,
            top: 0,
            right: w * aa,
            bottom: h * aa,
        },
        bg,
    );
    let _ = DeleteObject(bg);
    scene(&canvas, s, now, l);
    let _ = SetStretchBltMode(frame.dc, HALFTONE);
    let _ = SetBrushOrgEx(frame.dc, 0, 0, None);
    let _ = StretchBlt(
        frame.dc,
        0,
        0,
        w,
        h,
        shapes.dc,
        0,
        0,
        w * aa,
        h * aa,
        SRCCOPY,
    );
    scene(
        &Canvas {
            dc: frame.dc,
            layer: PaintLayer::Text,
            font_zoom: l.zoom,
            zoom: l.zoom,
            ..canvas
        },
        s,
        now,
        l,
    );
    Some(frame)
}
unsafe fn render_frame(
    target: HDC,
    w: i32,
    h: i32,
    dpi: u32,
    s: &Snapshot,
    now: std::time::SystemTime,
    open: bool,
    scroll: usize,
    glass: bool,
) {
    let l = Layout::new(w, h, open, dpi, scroll);
    let Some(frame) = draw_surface(target, &l, s, now, glass, false) else {
        return;
    };
    if glass {
        let Some(mask) = draw_surface(target, &l, s, now, true, true) else {
            return;
        };
        let _ = GdiFlush();
        for (pixel, coverage) in frame
            .pixels()
            .chunks_exact_mut(4)
            .zip(mask.pixels().chunks_exact(4))
        {
            let alpha = coverage[0].max(coverage[1]).max(coverage[2]);
            pixel[3] = alpha;
            for c in &mut pixel[..3] {
                *c = (*c).min(alpha);
            }
        }
    } else {
        let _ = GdiFlush();
        for pixel in frame.pixels().chunks_exact_mut(4) {
            pixel[3] = 255;
        }
    }
    let hwnd = WindowFromDC(target);
    if glass && !hwnd.is_invalid() {
        let mut rect = RECT::default();
        let _ = GetWindowRect(hwnd, &mut rect);
        let position = POINT {
            x: rect.left,
            y: rect.top,
        };
        let size = SIZE { cx: w, cy: h };
        let source = POINT::default();
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let _ = UpdateLayeredWindow(
            hwnd,
            HDC::default(),
            Some(&position),
            Some(&size),
            frame.dc,
            Some(&source),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        );
    } else {
        let _ = BitBlt(target, 0, 0, w, h, frame.dc, 0, 0, SRCCOPY);
    }
}
unsafe fn stop_motion_timer(hwnd: HWND) {
    let _ = KillTimer(hwnd, TIMER_MOTION);
    MOTION_TIMER_ACTIVE.store(false, Ordering::Relaxed);
}
unsafe fn paint(hwnd: HWND) {
    let light = LIGHT_THEME.load(Ordering::Relaxed);
    if MATERIAL_THEME.load(Ordering::Relaxed) != light as usize {
        MATERIAL_THEME.store(light as usize, Ordering::Relaxed);
        GLASS_ACTIVE.store(crate::clock_material::apply(hwnd, light), Ordering::Relaxed);
    }
    let mut ps = PAINTSTRUCT::default();
    let target = BeginPaint(hwnd, &mut ps);
    let mut r = RECT::default();
    let _ = GetClientRect(hwnd, &mut r);
    if r.right > 0 && r.bottom > 0 {
        select_available_provider();
        #[cfg(not(test))]
        let raw = crate::window::clock_snapshot(selected_provider());
        #[cfg(test)]
        let raw = TEST_SNAPSHOT
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| crate::window::clock_snapshot(selected_provider()));
        let now = std::time::SystemTime::now();
        let frame = MOTION
            .get_or_init(|| Mutex::new(crate::clock_motion::Motion::default()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .frame(&raw, now, std::time::Instant::now());
        #[cfg(test)]
        if TEST_SNAPSHOT.lock().unwrap().is_some() {
            TEST_FRAMES.lock().unwrap().push(frame.snapshot.clone());
        }
        if frame.animating {
            if !MOTION_TIMER_ACTIVE.swap(true, Ordering::Relaxed) {
                if SetTimer(hwnd, TIMER_MOTION, 16, None) == 0 {
                    MOTION_TIMER_ACTIVE.store(false, Ordering::Relaxed);
                }
            }
        } else {
            stop_motion_timer(hwnd);
        }
        render_frame(
            target,
            r.right,
            r.bottom,
            GetDpiForWindow(hwnd),
            &frame.snapshot,
            now,
            SIDEBAR_OPEN.load(Ordering::Relaxed),
            SIDE_SCROLL.load(Ordering::Relaxed),
            GLASS_ACTIVE.load(Ordering::Relaxed),
        );
    }
    let _ = EndPaint(hwnd, &ps);
}
fn scale(dpi: i32, value: i32) -> i32 {
    (value as f64 * dpi as f64 / 96.0).round() as i32
}

#[cfg(test)]
mod tests {
    use super::*;
    static UI_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());
    fn sample(now: std::time::SystemTime, remaining: f64) -> Snapshot {
        Snapshot {
            available: true,
            refresh_minutes: 15,
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
    fn compact_layout_uses_fixed_dpi_fonts_and_scrollable_controls() {
        let _guard = UI_TESTS.lock().unwrap();
        for dpi in [96, 120, 144, 192] {
            for (w, h) in [(360, 460), (420, 540), (600, 700)] {
                let z = dpi as f64 / 96.0;
                let l = Layout::new(
                    ((w + 288) as f64 * z) as i32,
                    (h as f64 * z) as i32,
                    true,
                    dpi,
                    0,
                );
                assert_eq!(font_pixels(12.0, l.zoom), (13.0 * z).round() as i32);
                assert!((l.main - w as f64).abs() < 1.0);
                for id in [1, 9, 2, 3, 4, 5, 8, 10, 11, 12] {
                    let r = l.control(id).unwrap();
                    assert_eq!(action_at(r.x + r.w / 2.0, r.y + r.h / 2.0, &l), id);
                }
                let bottom = Layout::new(
                    ((w + 288) as f64 * z) as i32,
                    (h as f64 * z) as i32,
                    true,
                    dpi,
                    9999,
                );
                assert!(bottom.control(7).unwrap().y + 34.0 <= bottom.height - 8.0);
                MENU_OPEN.store(true, Ordering::Relaxed);
                let m = bottom.menu();
                assert!(m.y >= 84.0 && m.y + m.h <= bottom.height - 8.0);
                for id in 20..=23 {
                    let r = bottom.control(id).unwrap();
                    assert_eq!(action_at(r.x + 8.0, r.y + 8.0, &bottom), id);
                }
                MENU_OPEN.store(false, Ordering::Relaxed);
            }
        }
    }
    #[test]
    fn native_window_preserves_clock_size_and_keyboard_reaches_refresh() {
        let _guard = UI_TESTS.lock().unwrap();
        unsafe {
            SIDEBAR_OPEN.store(false, Ordering::Relaxed);
            SIDE_SCROLL.store(0, Ordering::Relaxed);
            MENU_OPEN.store(false, Ordering::Relaxed);
            FOCUSED.store(0, Ordering::Relaxed);
            let class =
                native_interop::wide_str(&format!("CompactClockTest{}", std::process::id()));
            let instance = HINSTANCE(GetModuleHandleW(PCWSTR::null()).unwrap().0);
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(clock_proc),
                hInstance: instance,
                lpszClassName: PCWSTR(class.as_ptr()),
                ..Default::default()
            };
            assert_ne!(RegisterClassExW(&wc), 0);
            let hwnd = CreateWindowExW(
                WS_EX_APPWINDOW | WS_EX_LAYERED,
                PCWSTR(class.as_ptr()),
                PCWSTR(class.as_ptr()),
                WS_POPUP | WS_THICKFRAME | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX,
                20,
                20,
                420,
                540,
                HWND::default(),
                HMENU::default(),
                instance,
                None,
            )
            .unwrap();
            assert_eq!(GetWindowLongW(hwnd, GWL_STYLE) as u32 & WS_CAPTION.0, 0);
            let before = window_layout(hwnd);
            activate_action(hwnd, 1);
            let open = window_layout(hwnd);
            assert!((before.main - open.main).abs() < 1.0);
            assert!((before.height - open.height).abs() < 1.0);
            focus_visible(hwnd, 7);
            let l = window_layout(hwnd);
            assert!(l.control(7).unwrap().y + 34.0 <= l.height - 8.0);
            assert!(l.scroll > 0.0);
            activate_action(hwnd, 6);
            let l = window_layout(hwnd);
            assert!(l.menu().y + l.menu().h <= l.height - 8.0);
            let _ = SendMessageW(hwnd, WM_KEYDOWN, WPARAM(0x1b), LPARAM(0));
            assert!(!MENU_OPEN.load(Ordering::Relaxed));
            assert!(SIDEBAR_OPEN.load(Ordering::Relaxed));
            let _ = SendMessageW(hwnd, WM_KEYDOWN, WPARAM(0x1b), LPARAM(0));
            assert!(!SIDEBAR_OPEN.load(Ordering::Relaxed));
            activate_action(hwnd, 1);
            let l = window_layout(hwnd);
            let mut cursor = POINT {
                x: ((l.main + 40.0) * l.zoom) as i32,
                y: (200.0 * l.zoom) as i32,
            };
            let _ = ClientToScreen(hwnd, &mut cursor);
            let _ = SendMessageW(
                hwnd,
                WM_MOUSEWHEEL,
                WPARAM(((-120i16 as u16 as u32) << 16) as usize),
                LPARAM((((cursor.y as u32 & 65535) << 16) | (cursor.x as u32 & 65535)) as isize),
            );
            assert!(SIDE_SCROLL.load(Ordering::Relaxed) > 0);
            activate_action(hwnd, 1);
            activate_action(hwnd, 3);
            assert!(IsIconic(hwnd).as_bool());
            let _ = ShowWindow(hwnd, SW_RESTORE);
            activate_action(hwnd, 4);
            assert!(IsZoomed(hwnd).as_bool());
            activate_action(hwnd, 4);
            assert!(!IsZoomed(hwnd).as_bool());
            let theme = LIGHT_THEME.load(Ordering::Relaxed);
            activate_action(hwnd, 9);
            assert_ne!(LIGHT_THEME.load(Ordering::Relaxed), theme);
            activate_action(hwnd, 9);
            let _ = DestroyWindow(hwnd);
            let _ = UnregisterClassW(PCWSTR(class.as_ptr()), instance);
            FOCUSED.store(0, Ordering::Relaxed);
            SIDE_SCROLL.store(0, Ordering::Relaxed);
        }
    }
    #[test]
    fn real_window_refresh_retains_sectors_and_timer_paints_intermediate_frames() {
        let _guard = UI_TESTS.lock().unwrap();
        unsafe {
            *MOTION
                .get_or_init(|| Mutex::new(Default::default()))
                .lock()
                .unwrap() = Default::default();
            TEST_FRAMES.lock().unwrap().clear();
            let now = std::time::SystemTime::now();
            let initial = Snapshot {
                provider_name: "Codex".into(),
                updated_at: Some(now),
                ..sample(now, 63.0)
            };
            *TEST_SNAPSHOT.lock().unwrap() = Some(initial.clone());
            let class = native_interop::wide_str("ClockMotionRegression");
            let instance = HINSTANCE(GetModuleHandleW(PCWSTR::null()).unwrap().0);
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(clock_proc),
                hInstance: instance,
                lpszClassName: PCWSTR(class.as_ptr()),
                ..Default::default()
            };
            assert_ne!(RegisterClassExW(&wc), 0);
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                PCWSTR(class.as_ptr()),
                PCWSTR(class.as_ptr()),
                WS_POPUP,
                -2000,
                -2000,
                360,
                460,
                HWND::default(),
                HMENU::default(),
                instance,
                None,
            )
            .unwrap();
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            let repaint = || {
                let _ = InvalidateRect(hwnd, None, BOOL(0));
                let _ = UpdateWindow(hwnd);
            };
            repaint();
            for refreshing in [true, false] {
                *TEST_SNAPSHOT.lock().unwrap() = Some(Snapshot {
                    refreshing,
                    ahead_of_time: None,
                    status: "stale or failed".into(),
                    ..initial.clone()
                });
                repaint();
                let frames = TEST_FRAMES.lock().unwrap();
                let held = frames.last().unwrap();
                assert!(held.pace_cached);
                assert!(held.ahead_of_time.is_some());
                assert!(window_geometry(held, now).is_some());
                assert_eq!(held.remaining_percent, 63.0);
            }
            TEST_FRAMES.lock().unwrap().clear();
            *TEST_SNAPSHOT.lock().unwrap() = Some(Snapshot {
                remaining_percent: 20.0,
                updated_at: Some(now + std::time::Duration::from_secs(1)),
                ..initial
            });
            repaint();
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(1300);
            while std::time::Instant::now() < deadline {
                let mut msg = MSG::default();
                while PeekMessageW(&mut msg, hwnd, 0, 0, PM_REMOVE).as_bool() {
                    DispatchMessageW(&msg);
                }
                if !MOTION_TIMER_ACTIVE.load(Ordering::Relaxed) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            let frames = TEST_FRAMES.lock().unwrap().clone();
            *TEST_SNAPSHOT.lock().unwrap() = None;
            let _ = DestroyWindow(hwnd);
            let _ = UnregisterClassW(PCWSTR(class.as_ptr()), instance);
            assert!(
                frames.len() >= 8,
                "actual native paint frames: {}",
                frames.len()
            );
            assert!(
                frames
                    .iter()
                    .filter(|f| f.remaining_percent > 20.0 && f.remaining_percent < 63.0)
                    .count()
                    >= 6
            );
            assert!(frames
                .iter()
                .all(|f| window_geometry(f, now).is_some() && f.ahead_of_time.is_some()));
            assert_eq!(frames.last().unwrap().remaining_percent, 20.0);
            assert!(!MOTION_TIMER_ACTIVE.load(Ordering::Relaxed));
        }
    }
    #[test]
    #[ignore = "writes compact viewport images for visual inspection"]
    fn render_compact_visual_fixtures() {
        let _guard = UI_TESTS.lock().unwrap();
        unsafe {
            let now = std::time::SystemTime::now();
            let mut reference_header: Option<Vec<u8>> = None;
            for (name, w, h, dpi, open, light, scroll, remaining) in [
                ("compact", 420, 540, 96, false, false, 0, 63.0),
                ("compact-min", 360, 460, 96, false, false, 0, 63.0),
                ("compact-large", 600, 700, 96, false, false, 0, 63.0),
                ("compact-min-en", 360, 460, 96, false, true, 0, 63.0),
                ("compact-large-en", 600, 700, 96, false, true, 0, 63.0),
                ("compact-details", 708, 540, 96, true, false, 0, 63.0),
                (
                    "compact-details-bottom",
                    708,
                    540,
                    96,
                    true,
                    false,
                    9999,
                    63.0,
                ),
                ("compact-light", 420, 540, 96, false, true, 0, 20.0),
                ("compact-125", 525, 675, 120, false, false, 0, 63.0),
                ("compact-150", 630, 810, 144, false, false, 0, 63.0),
            ] {
                LIGHT_THEME.store(light, Ordering::Relaxed);
                FOCUSED.store(0, Ordering::Relaxed);
                HOVERED.store(0, Ordering::Relaxed);
                MENU_OPEN.store(false, Ordering::Relaxed);
                let dc = CreateCompatibleDC(HDC::default());
                let info = BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: w,
                        biHeight: -h,
                        biPlanes: 1,
                        biBitCount: 32,
                        biCompression: BI_RGB.0,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                let mut bits = std::ptr::null_mut();
                let bitmap =
                    CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, HANDLE::default(), 0)
                        .unwrap();
                let old = SelectObject(dc, bitmap);
                let mut snapshot = sample(now, remaining);
                snapshot.chinese = !name.ends_with("-en");
                render_frame(dc, w, h, dpi, &snapshot, now, open, scroll, false);
                let _ = GdiFlush();
                let pixels = std::slice::from_raw_parts(bits as *const u8, (w * h * 4) as usize);
                if name == "compact" || name == "compact-min" {
                    let crop: Vec<u8> = (12usize..34)
                        .flat_map(|y| {
                            pixels[(y * w as usize + 44) * 4..(y * w as usize + 118) * 4]
                                .iter()
                                .copied()
                        })
                        .collect();
                    if let Some(reference) = &reference_header {
                        assert_eq!(
                            &crop, reference,
                            "native text must not be resampled when the viewport shrinks"
                        );
                    } else {
                        reference_header = Some(crop);
                    }
                }
                let mut bytes = Vec::new();
                bytes.extend_from_slice(b"BM");
                bytes.extend_from_slice(&(54u32 + pixels.len() as u32).to_le_bytes());
                bytes.extend_from_slice(&[0; 4]);
                bytes.extend_from_slice(&54u32.to_le_bytes());
                bytes.extend_from_slice(&40u32.to_le_bytes());
                bytes.extend_from_slice(&w.to_le_bytes());
                bytes.extend_from_slice(&(-h).to_le_bytes());
                bytes.extend_from_slice(&1u16.to_le_bytes());
                bytes.extend_from_slice(&32u16.to_le_bytes());
                bytes.extend_from_slice(&[0; 24]);
                bytes.extend_from_slice(pixels);
                std::fs::create_dir_all("target/clock-review").unwrap();
                std::fs::write(format!("target/clock-review/{name}.bmp"), bytes).unwrap();
                let _ = SelectObject(dc, old);
                let _ = DeleteObject(bitmap);
                let _ = DeleteDC(dc);
            }
            LIGHT_THEME.store(false, Ordering::Relaxed);
        }
    }
}
