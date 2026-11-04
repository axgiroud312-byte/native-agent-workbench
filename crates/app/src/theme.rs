use std::ops::Range;

use gpui_kit::component::{ActiveTheme as _, Theme, ThemeMode};
use gpui_kit::{App, Hsla, Pixels, Rems, Size, px, rems, rgb, size};

pub const BASE_FONT: f32 = 14.0;
pub const MIN_SIDEBAR_WIDTH_PX: u16 = 240;
pub const MAX_SIDEBAR_WIDTH_PX: u16 = 320;

pub fn install(cx: &mut App) {
    Theme::change(ThemeMode::Dark, None, cx);
    Theme::update(cx, |theme| {
        theme.font_family = "Segoe UI".into();
        theme.font_size = px(BASE_FONT);
        theme.mono_font_family = "Consolas".into();
        theme.mono_font_size = px(13.0);

        // Spec Section 3 semantic charcoal tokens:
        // bg.window: #191B1D, bg.sidebar: #1D1F21, bg.workspace: #242629,
        // bg.surface: #2B2D30, bg.hover: #303236, bg.selected: #35383C,
        // border.subtle: #36393D, text.primary: #E6E7E9, text.secondary: #A4A8AD, text.muted: #777C82
        theme.background = rgb(0x242629).into();
        theme.foreground = rgb(0xE6E7E9).into();

        theme.sidebar = rgb(0x1D1F21).into();
        theme.sidebar_foreground = theme.foreground;
        theme.sidebar_border = rgb(0x36393D).into();
        theme.sidebar_accent = rgb(0x35383C).into();
        theme.sidebar_accent_foreground = theme.foreground;

        theme.title_bar = rgb(0x191B1D).into();
        theme.title_bar_border = rgb(0x36393D).into();

        theme.border = rgb(0x36393D).into();
        theme.input = rgb(0x36393D).into();

        theme.muted = rgb(0x2B2D30).into();
        theme.muted_foreground = rgb(0xA4A8AD).into();

        theme.secondary = rgb(0x2B2D30).into();
        theme.secondary_foreground = theme.foreground;
        theme.secondary_hover = rgb(0x303236).into();
        theme.secondary_active = rgb(0x35383C).into();

        theme.accent = rgb(0x35383C).into();
        theme.accent_foreground = theme.foreground;

        theme.popover = rgb(0x2B2D30).into();
        theme.popover_foreground = theme.foreground;

        theme.list_active = rgb(0x35383C).into();
        theme.list_active_border = rgb(0x36393D).into();
        theme.list_hover = rgb(0x303236).into();

        theme.primary = theme.foreground;
        theme.primary_foreground = rgb(0x191B1D).into();
        theme.primary_hover = rgb(0xF4F4F5).into();
        theme.primary_active = rgb(0xD5D7DA).into();

        theme.ring = rgb(0xA4A8AD).into();
        theme.radius = px(6.0);
        theme.radius_lg = px(10.0);
        theme.shadow = false;
    });
}

pub fn apply_zoom_percent(zoom_percent: u16, cx: &mut App) -> u16 {
    let clamped = zoom_percent.clamp(80, 175);
    let scaled_font = BASE_FONT * (clamped as f32 / 100.0);
    Theme::update(cx, |theme| {
        theme.font_size = px(scaled_font);
    });
    clamped
}

pub fn window_bg(cx: &App) -> Hsla {
    cx.theme().title_bar
}

pub fn sidebar_bg(cx: &App) -> Hsla {
    cx.theme().sidebar
}

pub fn workspace_bg(cx: &App) -> Hsla {
    cx.theme().background
}

pub fn surface_bg(cx: &App) -> Hsla {
    cx.theme().muted
}

pub fn subtle_border(cx: &App) -> Hsla {
    cx.theme().border
}

pub fn text_primary(cx: &App) -> Hsla {
    cx.theme().foreground
}

pub fn text_secondary(cx: &App) -> Hsla {
    cx.theme().muted_foreground
}

pub fn text_muted(_cx: &App) -> Hsla {
    rgb(0x777C82).into()
}

pub fn initial_window_size() -> Size<Pixels> {
    size(px(1240.0), px(800.0))
}

pub fn minimum_window_size() -> Size<Pixels> {
    size(px(980.0), px(640.0))
}

pub fn clamp_sidebar_width_logical(width_px: u16, window_width: Option<Pixels>) -> u16 {
    let max_from_window = window_width
        .map(|w| (f32::from(w) * 0.42).round() as u16)
        .unwrap_or(MAX_SIDEBAR_WIDTH_PX)
        .clamp(MIN_SIDEBAR_WIDTH_PX, MAX_SIDEBAR_WIDTH_PX);
    width_px.clamp(MIN_SIDEBAR_WIDTH_PX, max_from_window)
}

pub fn sidebar_width_pixels(logical_px: u16, window_width: Option<Pixels>) -> Pixels {
    let clamped = clamp_sidebar_width_logical(logical_px, window_width);
    px(clamped as f32)
}

pub fn sidebar_size_range() -> Range<Pixels> {
    px(MIN_SIDEBAR_WIDTH_PX as f32)..px(MAX_SIDEBAR_WIDTH_PX as f32)
}

pub fn header_height() -> Rems {
    rems(44.0 / BASE_FONT)
}

pub fn session_row_height() -> Rems {
    rems(36.0 / BASE_FONT)
}

pub fn sidebar_padding() -> Rems {
    rems(16.0 / BASE_FONT)
}

pub fn child_indent() -> Rems {
    rems(12.0 / BASE_FONT)
}

pub fn conversation_max_width() -> Rems {
    rems(880.0 / BASE_FONT)
}

pub fn control_radius() -> Rems {
    rems(6.0 / BASE_FONT)
}

pub fn panel_radius() -> Rems {
    rems(8.0 / BASE_FONT)
}

pub fn composer_radius() -> Rems {
    rems(10.0 / BASE_FONT)
}

pub fn settings_width(window: &gpui_kit::Window) -> Pixels {
    window.rem_size() * (640.0 / BASE_FONT)
}
