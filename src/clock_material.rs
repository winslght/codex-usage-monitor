//! Windows composition acrylic, matching Tiez's 50% surface-opacity mapping.
use crate::clock_palette::Palette;
use windows::core::{s, w};
use windows::Win32::Foundation::{BOOL, HWND};
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::UI::Controls::MARGINS;
#[repr(C)]
struct AccentPolicy {
    state: u32,
    flags: u32,
    tint: u32,
    animation: u32,
}
#[repr(C)]
struct AttributeData {
    attribute: u32,
    data: *mut std::ffi::c_void,
    size: usize,
}
/// 50 / 50 = 1 in Tiez; this is a surface setting, not global HWND opacity.
pub fn shell_alpha(light: bool) -> u8 {
    ((if light { 0.16 + 0.14 } else { 0.20 + 0.14 }) * 255.0f64).round() as u8
}

fn layer(color: u32, p: Palette, light: bool) -> Option<(u32, f64)> {
    let tint = if light { 0xf8fafc } else { 0x14181e };
    if color == p.window {
        Some((tint, 0.0))
    } else if color == p.sidebar {
        Some((if light { 0xffffff } else { tint }, 0.24))
    } else if color == p.face {
        Some((
            if light { 0xffffff } else { 0x080d14 },
            if light { 0.46 } else { 0.44 },
        ))
    } else if color == p.field {
        Some((0xffffff, if light { 0.58 } else { 0.16 }))
    } else if color == p.hover {
        Some((0xffffff, if light { 0.72 } else { 0.24 }))
    } else if color == p.selected {
        Some((p.accent, 0.35))
    } else if color == p.safe_surface {
        Some((p.safe, 0.19))
    } else if color == p.risk_surface {
        Some((p.risk, 0.19))
    } else if color == p.line {
        Some((if light { 0x162234 } else { 0xffffff }, 0.14))
    } else if color == p.track && light {
        Some((0x162234, 0.10))
    } else if color == p.strong || color == p.track {
        Some((if light { 0x162234 } else { 0xffffff }, 0.22))
    } else {
        None
    }
}
pub fn surface_alpha(color: u32, p: Palette, light: bool, opaque: bool) -> u8 {
    if opaque {
        return 255;
    }
    let base = f64::from(shell_alpha(light)) / 255.0;
    let Some((_, extra)) = layer(color, p, light) else {
        return 255;
    };
    ((base + (1.0 - base) * extra) * 255.0).round() as u8
}
pub fn paint_rgb(
    color: u32,
    p: Palette,
    light: bool,
    glass: bool,
    mask: bool,
    opaque: bool,
) -> u32 {
    let alpha = if glass {
        surface_alpha(color, p, light, opaque)
    } else {
        255
    };
    if mask {
        return u32::from(alpha) * 0x010101;
    }
    if !glass || opaque {
        return color;
    }
    let Some((overlay, extra)) = layer(color, p, light) else {
        return color;
    };
    let base = if light { 0xf8fafcu32 } else { 0x14181eu32 };
    let base_alpha = f64::from(shell_alpha(light)) / 255.0;
    [16, 8, 0].iter().fold(0, |result, shift| {
        let v = (((overlay >> shift) & 255) as f64 * extra
            + ((base >> shift) & 255) as f64 * base_alpha * (1.0 - extra))
            .round() as u32;
        result | (v.min(u32::from(alpha)) << shift)
    })
}
pub unsafe fn apply(hwnd: HWND, light: bool) -> bool {
    let dark = BOOL::from(!light);
    let none = 0xfffffffeu32;
    let corner = DWMWCP_ROUND;
    let _ = DwmSetWindowAttribute(
        hwnd,
        DWMWA_USE_IMMERSIVE_DARK_MODE,
        &dark as *const _ as _,
        std::mem::size_of::<BOOL>() as u32,
    );
    let border = DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR, &none as *const _ as _, 4);
    let _ = DwmSetWindowAttribute(
        hwnd,
        DWMWA_WINDOW_CORNER_PREFERENCE,
        &corner as *const _ as _,
        std::mem::size_of_val(&corner) as u32,
    );
    let applied = (|| -> Option<()> {
        let user = GetModuleHandleW(w!("user32.dll")).ok()?;
        let proc = GetProcAddress(user, s!("SetWindowCompositionAttribute"))?;
        let set: unsafe extern "system" fn(*mut std::ffi::c_void, *mut AttributeData) -> i32 =
            std::mem::transmute(proc);
        // The layered GDI host needs a brighter native light tint than WebView2.
        // Keep the reference 50% surface alpha; compensate only the OS backdrop.
        let tint = if light { 0xb8f8f6f6 } else { 0x601e1c1c };
        let mut policy = AccentPolicy {
            state: 4,
            flags: 0,
            tint,
            animation: 0,
        };
        let mut attr = AttributeData {
            attribute: 0x13,
            data: &mut policy as *mut _ as _,
            size: std::mem::size_of::<AccentPolicy>(),
        };
        if set(hwnd.0, &mut attr) == 0 {
            return None;
        }
        let margins = MARGINS {
            cxLeftWidth: -1,
            cxRightWidth: -1,
            cyTopHeight: -1,
            cyBottomHeight: -1,
        };
        DwmExtendFrameIntoClientArea(hwnd, &margins).ok()?;
        Some(())
    })()
    .is_some();
    crate::diagnose::log(format!(
        "clock acrylic applied={applied} border_hidden={} surface_opacity=50 shell_alpha={}",
        border.is_ok(),
        shell_alpha(light)
    ));
    if !applied {
        let _ = DwmExtendFrameIntoClientArea(hwnd, &MARGINS::default());
    }
    applied
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn opacity_matches_reference_50_percent_and_keeps_foreground_opaque() {
        assert_eq!(shell_alpha(true), 77);
        assert_eq!(shell_alpha(false), 87);
        for (light, p) in [
            (false, crate::clock_palette::DARK),
            (true, crate::clock_palette::LIGHT),
        ] {
            assert!(surface_alpha(p.window, p, light, false) < 128);
            assert_eq!(surface_alpha(p.text, p, light, false), 255);
            assert_eq!(surface_alpha(p.field, p, light, true), 255);
            let rgb = paint_rgb(p.window, p, light, true, false, false);
            let alpha = surface_alpha(p.window, p, light, false) as u32;
            assert!(
                (rgb & 255) <= alpha && ((rgb >> 8) & 255) <= alpha && ((rgb >> 16) & 255) <= alpha
            );
        }
    }
}
