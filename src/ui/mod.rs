//! Shared styling for the popup.
//!
//! Nothing here picks a colour, radius or font size by hand: everything comes
//! from the active COSMIC theme, so the menu follows light/dark, accent,
//! roundness and frosted glass without knowing which is in use. The tile
//! painting is adapted from cosmic-control-center-applet, where the reasons
//! behind each choice were found the hard way.

pub mod app_list;
pub mod context;
pub mod rail;
pub mod tiles;

use cosmic::iced::{Background, Border, Color, Length};
use cosmic::widget::button;

use crate::config::TileFinish;

pub const RAIL_WIDTH: f32 = 56.0;
pub const LIST_WIDTH: f32 = 300.0;
/// App icon size in a list row.
pub const ICON: u16 = 24;
/// Fixed so a letter jump can compute exact scroll offsets.
pub const ROW_HEIGHT: f32 = 36.0;
/// A letter header: an 8 px gap over a 24 px band with 2 below. Short on
/// purpose — the alphabet is the column's spine, so a letter has to cost
/// far less vertical room than the rows it labels.
pub const HEADER_HEIGHT: f32 = 34.0;
/// The band the header's own text sits in, inside `HEADER_HEIGHT`.
pub const HEADER_BAND: f32 = 24.0;
/// The one label over the pinned block: 6 above, 18, 4 below.
pub const ZONE_LABEL_HEIGHT: f32 = 28.0;
/// Left inset of a list row, to the icon.
pub const ROW_GUTTER: u16 = 12;
/// Right inset of a scrolling column: just enough for the overlay thumb to
/// ride beside the labels rather than over them.
pub const SCROLL_GUTTER: u16 = 8;
/// The overlay scroll thumb: a 4 px bar flush with the column's own right
/// edge, no permanent track.
pub const SCROLLBAR: f32 = 4.0;

/// Every scrolling column in the popup: a 6 px overlay thumb instead of the
/// 8 px always-on track libcosmic gives by default.
pub fn thin_scroll<'a, M: 'a>(
    s: cosmic::iced::widget::Scrollable<'a, M, cosmic::Theme, cosmic::Renderer>,
) -> cosmic::iced::widget::Scrollable<'a, M, cosmic::Theme, cosmic::Renderer> {
    s.class(cosmic::theme::iced::Scrollable::Minimal)
        .scrollbar_width(SCROLLBAR)
        .scroller_width(SCROLLBAR)
        // Inset by half its own width: the bar hugs the column edge instead
        // of floating a gutter's width inside the rows.
        .scrollbar_padding(SCROLLBAR / 2.0)
}

/// A muted caption heading: 12 px semibold at 60 % of the foreground, which
/// is how a semantic group ("Most used") tells itself apart from an index
/// letter without inventing a colour.
pub fn muted_text(theme: &cosmic::Theme) -> cosmic::iced::widget::text::Style {
    let mut c: Color = theme.cosmic().background(theme.transparent).on.into();
    c.a *= 0.6;
    cosmic::iced::widget::text::Style {
        color: Some(c),
        selected_fill: theme.cosmic().accent.base.into(),
    }
}

/// 13 px semibold: the weight a section or letter header carries.
pub fn header_text<'a>(
    label: impl Into<std::borrow::Cow<'a, str>> + 'a,
) -> cosmic::widget::Text<'a, cosmic::Theme, cosmic::Renderer> {
    cosmic::widget::text(label)
        .size(13.0)
        .line_height(cosmic::iced::widget::text::LineHeight::Absolute(
            18.0.into(),
        ))
        .font(cosmic::font::semibold())
}

/// A 1 px vertical hairline in the theme's divider colour: what separates
/// the rail from the list now that the rail has no surface of its own.
pub fn v_hairline<'a, M: 'a>() -> cosmic::Element<'a, M> {
    cosmic::widget::container(cosmic::widget::Space::new().width(Length::Fixed(1.0)))
        .width(Length::Fixed(1.0))
        .height(Length::Fill)
        .class(cosmic::theme::Container::Custom(Box::new(|theme| {
            let mut edge = theme
                .cosmic()
                .background(theme.transparent)
                .component
                .divider;
            edge.alpha *= 0.55;
            cosmic::widget::container::Style {
                background: Some(Background::Color(Color::from(edge))),
                ..Default::default()
            }
        })))
        .into()
}

/// The search field. libcosmic's own `Search` class paints a fully
/// saturated 2 px accent ring on focus, which under an orange accent is the
/// loudest thing on the menu; this keeps the accent but at 55 % alpha, over
/// a quiet 1 px edge at rest.
pub fn search_input_class() -> cosmic::theme::TextInput {
    use cosmic::widget::text_input::Appearance;

    fn base(theme: &cosmic::Theme, border_width: f32, border_color: Color) -> Appearance {
        let cosmic = theme.cosmic();
        let layer = cosmic.background(theme.transparent);
        let mut fill: Color = layer.component.base.into();
        fill.a *= 0.5;
        let mut hint: Color = layer.on.into();
        hint.a *= 0.55;
        Appearance {
            background: Background::Color(fill),
            border_radius: cosmic.corner_radii.radius_s.into(),
            border_offset: None,
            border_width,
            border_color,
            icon_color: None,
            text_color: None,
            placeholder_color: hint,
            selected_text_color: cosmic.on_accent_color().into(),
            selected_fill: cosmic.accent_color().into(),
            label_color: layer.on.into(),
        }
    }

    fn quiet(theme: &cosmic::Theme, scale: f32) -> Appearance {
        let mut edge: Color = theme
            .cosmic()
            .background(theme.transparent)
            .component
            .divider
            .into();
        edge.a *= scale;
        base(theme, 1.0, edge)
    }

    fn ring(theme: &cosmic::Theme) -> Appearance {
        let mut accent: Color = theme.cosmic().accent.base.into();
        // A hint of the accent, not a saturated band: under a warm accent
        // the old 2 px / 55 % ring was the loudest thing on the menu and
        // pulled the eye away from the list it sits over.
        accent.a *= 0.38;
        base(theme, 1.0, accent)
    }

    cosmic::theme::TextInput::Custom {
        active: Box::new(|theme| quiet(theme, 1.0)),
        error: Box::new(|theme| {
            let danger = theme.cosmic().destructive_color();
            base(theme, 2.0, Color::from(danger))
        }),
        hovered: Box::new(|theme| quiet(theme, 1.6)),
        focused: Box::new(ring),
        disabled: Box::new(|theme| quiet(theme, 0.5)),
    }
}

/// The popup's spacing scale, read from the theme. COSMIC ships three
/// densities with different values for the same token, so these are looked
/// up rather than assumed.
#[derive(Debug, Clone, Copy)]
pub struct Spacing {
    pub gap: u16,
    pub pad_y: u16,
    pub section: u16,
}

impl Spacing {
    pub fn from_theme(theme: &cosmic::Theme) -> Self {
        Self::from_tokens(theme.cosmic().spacing)
    }

    /// From the Appearance density setting alone, which is readable the
    /// moment the process starts; the theme arrives a little later.
    pub fn from_density() -> Self {
        Self::from_tokens(cosmic::cosmic_theme::Spacing::from(
            cosmic::config::interface_density(),
        ))
    }

    fn from_tokens(spacing: cosmic::cosmic_theme::Spacing) -> Self {
        Self {
            gap: spacing.space_xxs,
            pad_y: spacing.space_xxs,
            section: spacing.space_xs,
        }
    }
}

/// The colour any card on the popup is painted in: COSMIC's component on the
/// background layer. It is frost-aware (its alpha follows the frosted flags),
/// and it is *not* `Container::Primary`, which under frost is the popup's own
/// glass and makes cards read as holes.
fn tile_component(theme: &cosmic::Theme) -> cosmic::cosmic_theme::Component {
    theme
        .cosmic()
        .background(theme.transparent)
        .component
        .clone()
}

/// How much of the card colour a `Frosted` tile keeps: dense enough to read
/// as a surface, thin enough that the blur behind still shows.
const FROSTED_TILE_ALPHA: f32 = 0.88;

/// Fill and edge width for a finish. One place, so a button tile and a
/// container tile cannot disagree about what "frosted" means.
fn finish_paint(
    theme: &cosmic::Theme,
    finish: TileFinish,
    fill: cosmic::cosmic_theme::palette::Srgba,
) -> (Option<Color>, f32) {
    let mut fill = fill;
    match finish {
        TileFinish::Solid => (Some(Color::from(fill)), 1.0),
        TileFinish::Frosted => {
            fill.alpha *= FROSTED_TILE_ALPHA;
            (Some(Color::from(fill)), 1.0)
        }
        TileFinish::Outline => (None, 1.0),
        TileFinish::Accent => (Some(Color::from(theme.cosmic().accent_color())), 0.0),
    }
}

/// Tiles take the desktop's medium radius: 16 on Round, 8 on Slightly round,
/// 2 on Square. A pill (CCCA's rule) would turn a 2×2 tile into a circle.
pub fn tile_radius(theme: &cosmic::Theme) -> f32 {
    theme.cosmic().corner_radii.radius_m[0]
}

/// Rows, rail buttons and menu items take the small radius.
pub fn row_radius(theme: &cosmic::Theme) -> f32 {
    theme.cosmic().corner_radii.radius_s[0]
}

/// The 2 px accent ring a keyboard-focused control wears. Drawn as the
/// button's outline so it sits *outside* the shape and never eats the fill.
fn focus_ring(style: &mut button::Style, theme: &cosmic::Theme) {
    let accent = theme.cosmic().accent.base;
    style.outline_width = 2.0;
    style.outline_color = Color::from(accent);
}

fn tile_border(theme: &cosmic::Theme, width: f32) -> Border {
    let mut edge = theme
        .cosmic()
        .background(theme.transparent)
        .component
        .divider;
    edge.alpha *= 0.7;
    Border {
        radius: tile_radius(theme).into(),
        width,
        color: if width > 0.0 {
            Color::from(edge)
        } else {
            Color::TRANSPARENT
        },
    }
}

/// `#rrggbb` (or `#rgb`) to a colour; anything else is `None`.
pub fn parse_hex(s: &str) -> Option<Color> {
    let hex = s.trim().strip_prefix('#')?;
    // `hex.len()` counts bytes, so a multibyte character in a 3- or 6-byte
    // string would land the `&hex[..]` slices mid-character and panic a
    // resident applet. Colour literals are ASCII; reject anything else first.
    if !hex.is_ascii() {
        return None;
    }
    let byte = |a: &str| u8::from_str_radix(a, 16).ok();
    let (r, g, b) = match hex.len() {
        6 => (byte(&hex[0..2])?, byte(&hex[2..4])?, byte(&hex[4..6])?),
        3 => {
            let d = |i: usize| byte(&hex[i..=i]).map(|v| v * 17);
            (d(0)?, d(1)?, d(2)?)
        }
        _ => return None,
    };
    Some(Color::from_rgb8(r, g, b))
}

/// Whether black text reads better than white on `fill` (WCAG-ish luma).
fn is_light(c: Color) -> bool {
    0.299 * c.r + 0.587 * c.g + 0.114 * c.b > 0.6
}

/// A tile filled with the user's own colour: the colour at rest, a touch
/// lighter with a bright inset edge under the pointer, a darker press, and
/// black or white content by its luma.
pub fn colored_tile_class(fill: Color) -> button::ButtonClass {
    /// The inset edge a hovered or keyboard-focused tile wears. Drawn as
    /// the border rather than a wash so hover reads on a dark fill and a
    /// light one alike.
    const EDGE: Color = Color {
        r: 1.0,
        g: 1.0,
        b: 1.0,
        a: 0.45,
    };

    fn style(theme: &cosmic::Theme, fill: Color, edge: Option<Color>) -> button::Style {
        let content = if is_light(fill) {
            Color::BLACK
        } else {
            Color::WHITE
        };
        button::Style {
            background: Some(Background::Color(fill)),
            border_radius: tile_radius(theme).into(),
            border_width: if edge.is_some() { 2.0 } else { 0.0 },
            border_color: edge.unwrap_or(Color::TRANSPARENT),
            text_color: Some(content),
            icon_color: Some(content),
            ..button::Style::new()
        }
    }
    fn lighten(c: Color, by: f32) -> Color {
        Color {
            r: c.r + (1.0 - c.r) * by,
            g: c.g + (1.0 - c.g) * by,
            b: c.b + (1.0 - c.b) * by,
            a: c.a,
        }
    }
    fn darken(c: Color, by: f32) -> Color {
        Color {
            r: c.r * (1.0 - by),
            g: c.g * (1.0 - by),
            b: c.b * (1.0 - by),
            a: c.a,
        }
    }
    // A light fill has no headroom to lighten into, so it darkens instead;
    // either way the hover is a visible step from rest.
    fn nudge(c: Color, by: f32) -> Color {
        if is_light(c) {
            darken(c, by)
        } else {
            lighten(c, by)
        }
    }

    button::ButtonClass::Custom {
        active: Box::new(move |focused, theme| {
            // Keyboard focus keeps the inset edge *and* gains the accent
            // ring outside the shape, so focus and hover stay apart.
            let mut s = style(theme, fill, focused.then_some(EDGE));
            if focused {
                focus_ring(&mut s, theme);
            }
            s
        }),
        disabled: Box::new(move |theme| style(theme, fill, None)),
        hovered: Box::new(move |_focused, theme| style(theme, nudge(fill, 0.08), Some(EDGE))),
        pressed: Box::new(move |_focused, theme| style(theme, darken(fill, 0.12), Some(EDGE))),
    }
}

/// A tile over its own picture: nothing at rest so the picture shows, the
/// card's washes under the pointer, and the tile radius throughout.
pub fn image_tile_class() -> button::ButtonClass {
    fn base(
        theme: &cosmic::Theme,
        fill: Option<cosmic::cosmic_theme::palette::Srgba>,
    ) -> button::Style {
        button::Style {
            background: fill.map(|c| Background::Color(Color::from(c))),
            border_radius: tile_radius(theme).into(),
            text_color: None,
            icon_color: None,
            ..button::Style::new()
        }
    }

    button::ButtonClass::Custom {
        active: Box::new(|_focused, theme| base(theme, None)),
        disabled: Box::new(|theme| base(theme, None)),
        hovered: Box::new(|_focused, theme| base(theme, Some(tile_component(theme).hover))),
        pressed: Box::new(|_focused, theme| base(theme, Some(tile_component(theme).pressed))),
    }
}

/// A tile button in the frost-aware card colour, with that component's hover
/// and pressed washes. Text and icon colours are left unset so they inherit.
pub fn tile_button_class(finish: TileFinish) -> button::ButtonClass {
    fn style(
        theme: &cosmic::Theme,
        finish: TileFinish,
        fill: cosmic::cosmic_theme::palette::Srgba,
    ) -> button::Style {
        let (fill, edge) = finish_paint(theme, finish, fill);
        let border = tile_border(theme, edge);
        button::Style {
            background: fill.map(Background::Color),
            border_radius: border.radius,
            border_width: border.width,
            border_color: border.color,
            // Accent is the one finish with its own foreground: text on the
            // accent fill must use the theme's on-accent colour to stay legible.
            text_color: (finish == TileFinish::Accent)
                .then(|| Color::from(theme.cosmic().on_accent_color())),
            icon_color: (finish == TileFinish::Accent)
                .then(|| Color::from(theme.cosmic().on_accent_color())),
            ..button::Style::new()
        }
    }

    button::ButtonClass::Custom {
        active: Box::new(move |focused, theme| {
            let mut s = style(theme, finish, tile_component(theme).base);
            if focused {
                focus_ring(&mut s, theme);
            }
            s
        }),
        disabled: Box::new(move |theme| style(theme, finish, tile_component(theme).disabled)),
        hovered: Box::new(move |_focused, theme| {
            style(theme, TileFinish::Solid, tile_component(theme).hover)
        }),
        pressed: Box::new(move |_focused, theme| {
            style(theme, TileFinish::Solid, tile_component(theme).pressed)
        }),
    }
}

/// No fill at rest, the card component's washes under the pointer. Used for
/// list rows, rail buttons and menu items. `Transparent` is no good: it zeroes
/// the text colour rather than leaving it alone.
pub fn quiet_button() -> button::ButtonClass {
    fn base(
        theme: &cosmic::Theme,
        fill: Option<cosmic::cosmic_theme::palette::Srgba>,
    ) -> button::Style {
        button::Style {
            background: fill.map(|c| Background::Color(Color::from(c))),
            border_radius: row_radius(theme).into(),
            text_color: None,
            icon_color: None,
            ..button::Style::new()
        }
    }

    button::ButtonClass::Custom {
        active: Box::new(|focused, theme| {
            let mut s = base(theme, None);
            if focused {
                focus_ring(&mut s, theme);
            }
            s
        }),
        disabled: Box::new(|theme| base(theme, None)),
        hovered: Box::new(|_focused, theme| base(theme, Some(tile_component(theme).hover))),
        pressed: Box::new(|_focused, theme| base(theme, Some(tile_component(theme).pressed))),
    }
}

/// The keyboard-selected search result: an accent tint, like COSMIC's own
/// list selection, with the text left in its normal colour.
pub fn selected_button() -> button::ButtonClass {
    fn base(theme: &cosmic::Theme, alpha: f32) -> button::Style {
        let mut tint = theme.cosmic().accent_color();
        tint.alpha = alpha;
        button::Style {
            background: Some(Background::Color(Color::from(tint))),
            border_radius: row_radius(theme).into(),
            text_color: None,
            icon_color: None,
            ..button::Style::new()
        }
    }

    button::ButtonClass::Custom {
        active: Box::new(|_focused, theme| base(theme, 0.24)),
        disabled: Box::new(|theme| base(theme, 0.24)),
        hovered: Box::new(|_focused, theme| base(theme, 0.32)),
        pressed: Box::new(|_focused, theme| base(theme, 0.4)),
    }
}

/// A floating menu's card: the popup's own layer colour, opaque enough to
/// read over the list beneath it, with the theme's medium radius.
pub fn menu_card<'a>() -> cosmic::theme::Container<'a> {
    cosmic::theme::Container::Custom(Box::new(|theme| {
        let cosmic = theme.cosmic();
        let layer = cosmic.background(false);
        let mut bg = layer.base;
        bg.alpha = bg.alpha.max(0.96);
        cosmic::widget::container::Style {
            background: Some(Background::Color(Color::from(bg))),
            text_color: Some(layer.on.into()),
            icon_color: Some(layer.on.into()),
            border: Border {
                radius: cosmic.corner_radii.radius_m.into(),
                width: 1.0,
                color: Color::from(layer.divider),
            },
            ..Default::default()
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_parses_long_short_and_rejects_junk() {
        let c = parse_hex("#E81123").unwrap();
        assert!((c.r - 232.0 / 255.0).abs() < 1e-5);
        assert!((c.b - 35.0 / 255.0).abs() < 1e-5);
        let short = parse_hex(" #f0a ").unwrap();
        assert!((short.r - 1.0).abs() < 1e-5);
        assert!((short.g - 0.0).abs() < 1e-5);
        for junk in ["", "#", "#12345", "red", "#gggggg", "e81123"] {
            assert!(parse_hex(junk).is_none(), "{junk:?}");
        }
    }

    #[test]
    fn a_multibyte_colour_is_rejected_not_a_panic() {
        // 3 and 6 bytes — the two lengths that get sliced — with a multibyte
        // character straddling a slice boundary, where byte slicing panicked.
        for junk in ["#a\u{e9}", "#a\u{e9}bcd"] {
            assert!(parse_hex(junk).is_none(), "{junk:?}");
        }
    }

    #[test]
    fn light_fills_take_black_content_dark_take_white() {
        assert!(is_light(Color::from_rgb8(255, 185, 0)));
        assert!(!is_light(Color::from_rgb8(0, 120, 215)));
    }
}
