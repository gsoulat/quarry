//! Thème sombre de Quarry et styles des panneaux.

use iced::widget::{button, container};
use iced::{Background, Border, Color, Theme, color};

pub const BACKGROUND: Color = color!(0x0e1116);
pub const SURFACE: Color = color!(0x161b22);
pub const SURFACE_ALT: Color = color!(0x1c2230);
pub const BORDER: Color = color!(0x262d3a);
pub const TEXT: Color = color!(0xe6eaf2);
pub const MUTED: Color = color!(0x8b95a7);
pub const PRIMARY: Color = color!(0x7aa2f7);
pub const SUCCESS: Color = color!(0x9ece6a);
pub const WARNING: Color = color!(0xe0af68);
pub const DANGER: Color = color!(0xf7768e);

const RADIUS: f32 = 8.0;

pub fn quarry() -> Theme {
    Theme::custom(
        "Quarry",
        iced::theme::Palette {
            background: BACKGROUND,
            text: TEXT,
            primary: PRIMARY,
            success: SUCCESS,
            warning: WARNING,
            danger: DANGER,
        },
    )
}

/// Panneau principal (barre latérale, éditeur, résultats).
pub fn panel(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(SURFACE)),
        border: Border { color: BORDER, width: 1.0, radius: RADIUS.into() },
        ..container::Style::default()
    }
}

/// En-tête de colonne dans les grilles.
pub fn header_cell(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(SURFACE_ALT)),
        text_color: Some(MUTED),
        ..container::Style::default()
    }
}

/// Ligne de grille, avec une alternance de fond pour guider l'œil.
pub fn grid_row(odd: bool) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: odd.then_some(Background::Color(color!(0x131820))),
        ..container::Style::default()
    }
}

/// Onglet ou entrée de liste : discret, mis en avant quand il est actif.
pub fn list_item(active: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, status| {
        let background = match (active, status) {
            (true, _) => Some(Background::Color(SURFACE_ALT)),
            (false, button::Status::Hovered) => Some(Background::Color(color!(0x1a202b))),
            _ => None,
        };
        button::Style {
            background,
            text_color: if active { TEXT } else { MUTED },
            border: Border { radius: 6.0.into(), ..Border::default() },
            ..button::Style::default()
        }
    }
}
