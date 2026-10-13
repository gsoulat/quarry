//! Thème sombre de Quarry et styles des panneaux.

use iced::widget::{button, container, pane_grid as grid};
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

/// Poignée entre les panneaux : visible seulement au survol.
pub fn pane_grid(_theme: &Theme) -> grid::Style {
    let line = |color| grid::Line { color, width: 3.0 };
    grid::Style {
        hovered_region: grid::Highlight {
            background: Background::Color(Color { a: 0.1, ..PRIMARY }),
            border: Border::default(),
        },
        picked_split: line(PRIMARY),
        hovered_split: line(Color { a: 0.6, ..PRIMARY }),
    }
}

/// Barre d'état en bas de la fenêtre.
pub fn status_bar(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(SURFACE)),
        border: Border { color: BORDER, width: 1.0, radius: 6.0.into() },
        ..container::Style::default()
    }
}

/// Bouton à contour, pour une action secondaire mais qui doit se voir
/// (« Déconnecter »). La couleur donne l'intention.
pub fn outline(color: Color) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        // Désactivé : gris et sans relief, pour qu'on voie qu'il n'y a rien à faire.
        let color = if matches!(status, button::Status::Disabled) { Color { a: 0.35, ..MUTED } } else { color };
        button::Style {
            background: hovered.then_some(Background::Color(Color { a: 0.15, ..color })),
            text_color: color,
            border: Border { color, width: 1.0, radius: 6.0.into() },
            ..button::Style::default()
        }
    }
}

/// Ligne de grille sélectionnée.
pub fn grid_row_selected(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color { a: 0.22, ..PRIMARY })),
        ..container::Style::default()
    }
}

/// Cellule de grille cliquable (copie la valeur), sans décoration.
pub fn cell(_theme: &Theme, status: button::Status) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: hovered.then_some(Background::Color(Color { a: 0.12, ..PRIMARY })),
        text_color: TEXT,
        border: Border { radius: 3.0.into(), ..Border::default() },
        ..button::Style::default()
    }
}

/// Infobulle.
pub fn tooltip(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(SURFACE_ALT)),
        text_color: Some(TEXT),
        border: Border { color: BORDER, width: 1.0, radius: 6.0.into() },
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
