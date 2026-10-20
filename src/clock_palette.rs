//! Semantic paint tokens inspired by Tiez's native theme separation.
//! Keep window chrome neutral; reserve semantic colors for quota information.
#[derive(Clone, Copy)]
pub struct Palette {
    pub window: u32,
    pub sidebar: u32,
    pub field: u32,
    pub hover: u32,
    pub text: u32,
    pub muted: u32,
    pub line: u32,
    pub strong: u32,
    pub accent: u32,
    pub selected: u32,
    pub disabled: u32,
    pub face: u32,
    pub tick: u32,
    pub track: u32,
    pub time: u32,
    pub safe: u32,
    pub risk: u32,
    pub warning: u32,
    pub safe_surface: u32,
    pub risk_surface: u32,
}
pub const DARK: Palette = Palette {
    window: 0x181b22,
    sidebar: 0x20252e,
    field: 0x272e38,
    hover: 0x303b48,
    text: 0xe6ebf3,
    muted: 0xa0aaba,
    line: 0x303946,
    strong: 0x465365,
    accent: 0x92bff2,
    selected: 0x2d415b,
    disabled: 0x626d7b,
    face: 0x141920,
    tick: 0x667182,
    track: 0x4a5969,
    time: 0x89cedc,
    safe: 0x78d9ac,
    risk: 0xed8a94,
    warning: 0xefbb82,
    safe_surface: 0x20392f,
    risk_surface: 0x3b2830,
};
pub const LIGHT: Palette = Palette {
    window: 0xf5f6f8,
    sidebar: 0xeceff3,
    field: 0xffffff,
    hover: 0xe4eaf2,
    text: 0x222a37,
    muted: 0x45566b,
    line: 0xd7dde5,
    strong: 0xbac5d2,
    accent: 0x336fac,
    selected: 0xdce8f7,
    disabled: 0x788391,
    face: 0xfcfdff,
    tick: 0x64758a,
    track: 0xc3cddb,
    time: 0x237684,
    safe: 0x1a7856,
    risk: 0xb83b4d,
    warning: 0x9a591e,
    safe_surface: 0xe2f1e9,
    risk_surface: 0xf8e4e8,
};
pub fn blend(a: u32, b: u32, weight: f64) -> u32 {
    [16, 8, 0].iter().fold(0, |value, shift| {
        value
            | (((((a >> shift) & 255) as f64 * weight
                + ((b >> shift) & 255) as f64 * (1.0 - weight))
                .round() as u32)
                << shift)
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    fn luminance(c: u32) -> f64 {
        [16, 8, 0]
            .iter()
            .zip([0.2126, 0.7152, 0.0722])
            .map(|(s, w)| {
                let v = ((c >> s) & 255) as f64 / 255.0;
                w * if v <= 0.04045 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            })
            .sum()
    }
    #[test]
    fn both_palettes_keep_primary_and_secondary_text_readable() {
        for p in [DARK, LIGHT] {
            for background in [p.window, p.sidebar, p.field] {
                for foreground in [p.text, p.muted] {
                    let (a, b) = (luminance(foreground), luminance(background));
                    assert!((a.max(b) + 0.05) / (a.min(b) + 0.05) >= 4.5);
                }
            }
        }
    }
}
