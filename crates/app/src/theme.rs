use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{App, Pixels, Rems, Size, px, rems, rgb, size};

const BASE_FONT: f32 = 14.0;

pub fn install(cx: &mut App) {
    Theme::change(ThemeMode::Dark, None, cx);
    Theme::update(cx, |theme| {
        theme.font_family = "Segoe UI".into();
        theme.font_size = px(BASE_FONT);
        theme.background = rgb(0x242629).into();
        theme.foreground = rgb(0xE6E7E9).into();
        theme.sidebar = rgb(0x1D1F21).into();
        theme.sidebar_foreground = theme.foreground;
        theme.sidebar_border = rgb(0x36393D).into();
        theme.border = theme.sidebar_border;
        theme.title_bar = theme.background;
        theme.title_bar_border = theme.background;
        theme.muted = rgb(0x2B2D30).into();
        theme.muted_foreground = rgb(0xA4A8AD).into();
        theme.secondary = theme.muted;
        theme.secondary_foreground = theme.foreground;
        theme.secondary_hover = rgb(0x303236).into();
        theme.secondary_active = rgb(0x35383C).into();
        theme.primary = theme.foreground;
        theme.primary_foreground = theme.background;
        theme.primary_hover = rgb(0xF4F4F5).into();
        theme.primary_active = rgb(0xD5D7DA).into();
        theme.ring = rgb(0xA4A8AD).into();
        theme.radius = px(6.0);
        theme.radius_lg = px(10.0);
        theme.shadow = false;
    });
}

// Window bounds are platform geometry; content geometry follows the rem scale.
pub fn initial_window_size() -> Size<Pixels> {
    size(px(1240.0), px(800.0))
}

pub fn minimum_window_size() -> Size<Pixels> {
    size(px(980.0), px(640.0))
}

pub fn sidebar_width() -> Rems {
    rems(264.0 / BASE_FONT)
}
