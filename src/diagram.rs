//! Diagramme des tables et de leurs relations, dessiné sur un canvas.
//!
//! Trois niveaux de lecture à partir du même graphe :
//! - **MPD** (physique) : ce que la base contient, types compris ;
//! - **MLD** (logique) : tables, clés primaires et étrangères ;
//! - **MCD** (conceptuel) : entités et associations avec cardinalités,
//!   obtenu par rétro-ingénierie (les clés étrangères redeviennent des
//!   associations, les tables de jonction des associations n-n).

use std::cell::Cell as StdCell;
use std::collections::{HashMap, HashSet};

use iced::mouse;
use iced::widget::canvas::{self, Frame, Geometry, Path, Stroke, Text};
use iced::{Color, Event, Font, Point, Rectangle, Size, Vector, alignment};

use crate::connector::{ColumnInfo, Relation, TableRef};
use crate::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Physical,
    Logical,
    Conceptual,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Physical, Mode::Logical, Mode::Conceptual];

    pub fn label(self) -> &'static str {
        match self {
            Mode::Physical => "MPD",
            Mode::Logical => "MLD",
            Mode::Conceptual => "MCD",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Mode::Physical => "Modèle physique : tables, colonnes et types tels que la base les contient.",
            Mode::Logical => "Modèle logique : tables, clés primaires (PK) et étrangères (FK), sans les types.",
            Mode::Conceptual => "Modèle conceptuel (rétro-ingénierie) : entités et associations avec cardinalités. \
                Les tables dont la clé n'est faite que de clés étrangères deviennent des associations.",
        }
    }
}

/// Ce que l'utilisateur fait sur le canvas ; l'application met l'état à jour.
#[derive(Debug, Clone)]
pub enum Interaction {
    MoveNode { index: usize, to: Point },
    Pan(Vector),
    Zoom { factor: f32, at: Point },
    Select(Option<usize>),
}

const FONT: Font = Font::MONOSPACE;
const FONT_SIZE: f32 = 12.0;
const CHAR_WIDTH: f32 = 7.2;
const HEADER_HEIGHT: f32 = 26.0;
const ROW_HEIGHT: f32 = 18.0;
const PADDING: f32 = 10.0;
const MIN_ZOOM: f32 = 0.2;
const MAX_ZOOM: f32 = 3.0;

#[derive(Debug, Clone)]
pub struct Node {
    pub table: TableRef,
    pub columns: Vec<ColumnInfo>,
    pub position: Point,
    /// Taille dans le mode courant.
    size: Size,
    /// Lignes affichées dans le mode courant.
    lines: Vec<String>,
    /// Table de jonction (association n-n en MCD).
    junction: bool,
}

impl Node {
    fn rect(&self) -> Rectangle {
        Rectangle::new(self.position, self.size)
    }

    fn center(&self) -> Point {
        self.position + Vector::new(self.size.width / 2.0, self.size.height / 2.0)
    }
}

pub struct Diagram {
    pub nodes: Vec<Node>,
    pub relations: Vec<Relation>,
    pub mode: Mode,
    pub zoom: f32,
    pub offset: Vector,
    pub selected: Option<usize>,
    /// Indices des nœuds par table, pour retrouver les extrémités des relations.
    index: HashMap<TableRef, usize>,
    /// Colonnes qui font partie d'une clé étrangère, par table.
    fk_columns: HashMap<TableRef, HashSet<String>>,
    cache: canvas::Cache,
    /// Dernière taille connue du canvas, pour « Ajuster ».
    viewport: StdCell<Size>,
}

impl Diagram {
    pub fn new(tables: Vec<(TableRef, Vec<ColumnInfo>)>, relations: Vec<Relation>) -> Self {
        let mut fk_columns: HashMap<TableRef, HashSet<String>> = HashMap::new();
        for rel in &relations {
            fk_columns.entry(rel.from.clone()).or_default().extend(rel.from_columns.iter().cloned());
        }

        let nodes = tables
            .into_iter()
            .map(|(table, columns)| {
                let fks = fk_columns.get(&table);
                let junction = is_junction(&columns, fks, relations.iter().filter(|r| r.from == table).count());
                Node { table, columns, position: Point::ORIGIN, size: Size::ZERO, lines: Vec::new(), junction }
            })
            .collect();

        let mut diagram = Self {
            nodes,
            relations,
            mode: Mode::Logical,
            zoom: 1.0,
            offset: Vector::ZERO,
            selected: None,
            index: HashMap::new(),
            fk_columns,
            cache: canvas::Cache::new(),
            viewport: StdCell::new(Size::new(1000.0, 700.0)),
        };
        diagram.index = diagram.nodes.iter().enumerate().map(|(i, n)| (n.table.clone(), i)).collect();
        diagram.set_mode(Mode::Logical);
        diagram.auto_layout();
        diagram
    }

    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
        for node in &mut self.nodes {
            let fks = self.fk_columns.get(&node.table);
            node.lines = describe(mode, &node.columns, fks, node.junction);
            let widest = node.lines.iter().map(String::len).max().unwrap_or(0).max(node.table.name.len() + 2);
            node.size = Size::new(
                (widest as f32 * CHAR_WIDTH + 2.0 * PADDING).max(120.0),
                HEADER_HEIGHT + node.lines.len() as f32 * ROW_HEIGHT + PADDING,
            );
        }
        self.cache.clear();
    }

    pub fn apply(&mut self, interaction: Interaction) {
        match interaction {
            Interaction::MoveNode { index, to } => {
                if let Some(node) = self.nodes.get_mut(index) {
                    node.position = to;
                }
            }
            Interaction::Pan(delta) => self.offset = self.offset + delta,
            Interaction::Zoom { factor, at } => {
                let zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
                let factor = zoom / self.zoom;
                // Le point sous le curseur reste fixe.
                self.offset = Vector::new(
                    at.x - (at.x - self.offset.x) * factor,
                    at.y - (at.y - self.offset.y) * factor,
                );
                self.zoom = zoom;
            }
            Interaction::Select(index) => self.selected = index,
        }
        self.cache.clear();
    }

    /// Placement par forces : les tables liées s'attirent, toutes se repoussent.
    pub fn auto_layout(&mut self) {
        let n = self.nodes.len();
        if n == 0 {
            return;
        }
        // Départ en grille pour ne pas dépendre du hasard.
        let cols = (n as f32).sqrt().ceil() as usize;
        for (i, node) in self.nodes.iter_mut().enumerate() {
            node.position = Point::new((i % cols) as f32 * 260.0, (i / cols) as f32 * 200.0);
        }

        let edges: Vec<(usize, usize)> = self
            .relations
            .iter()
            .filter_map(|r| Some((*self.index.get(&r.from)?, *self.index.get(&r.to)?)))
            .filter(|(a, b)| a != b)
            .collect();

        let k = 230.0_f32;
        let mut temperature = 120.0_f32;
        for _ in 0..300 {
            let centers: Vec<Point> = self.nodes.iter().map(Node::center).collect();
            let mut forces = vec![Vector::ZERO; n];

            for i in 0..n {
                for j in (i + 1)..n {
                    let d = centers[i] - centers[j];
                    // Les boîtes ont une taille : on retire la moitié des largeurs.
                    let margin = (self.nodes[i].size.width + self.nodes[j].size.width) / 2.0;
                    let dist = (d.x * d.x + d.y * d.y).sqrt().max(1.0);
                    let effective = (dist - margin * 0.6).max(10.0);
                    let push = k * k / effective / dist;
                    forces[i] = forces[i] + d * push;
                    forces[j] = forces[j] - d * push;
                }
            }
            for &(a, b) in &edges {
                let d = centers[a] - centers[b];
                let dist = (d.x * d.x + d.y * d.y).sqrt().max(1.0);
                let pull = dist * dist / k / dist * 0.6;
                forces[a] = forces[a] - d * pull;
                forces[b] = forces[b] + d * pull;
            }
            // Légère gravité vers le centre pour garder les îlots groupés.
            let cx = centers.iter().map(|p| p.x).sum::<f32>() / n as f32;
            let cy = centers.iter().map(|p| p.y).sum::<f32>() / n as f32;
            for (i, c) in centers.iter().enumerate() {
                forces[i] = forces[i] + Vector::new(cx - c.x, cy - c.y) * 0.02;
            }

            for (node, force) in self.nodes.iter_mut().zip(forces) {
                let len = (force.x * force.x + force.y * force.y).sqrt().max(0.001);
                let step = len.min(temperature);
                node.position = node.position + force * (step / len);
            }
            temperature = (temperature * 0.97).max(2.0);
        }

        self.fit();
    }

    /// Zoom et décalage pour voir tout le diagramme.
    pub fn fit(&mut self) {
        let Some(bounds) = self.bounds() else { return };
        let view = self.viewport.get();
        let zoom = (view.width / (bounds.width + 80.0))
            .min(view.height / (bounds.height + 80.0))
            .clamp(MIN_ZOOM, 1.2);
        self.zoom = zoom;
        self.offset = Vector::new(
            (view.width - bounds.width * zoom) / 2.0 - bounds.x * zoom,
            (view.height - bounds.height * zoom) / 2.0 - bounds.y * zoom,
        );
        self.cache.clear();
    }

    fn bounds(&self) -> Option<Rectangle> {
        let first = self.nodes.first()?.rect();
        Some(self.nodes.iter().skip(1).fold(first, |acc, n| acc.union(&n.rect())))
    }

    fn to_world(&self, screen: Point) -> Point {
        Point::new((screen.x - self.offset.x) / self.zoom, (screen.y - self.offset.y) / self.zoom)
    }

    fn node_at(&self, world: Point) -> Option<usize> {
        // Le dernier dessiné est au-dessus : on cherche en partant de la fin.
        self.nodes.iter().rposition(|n| n.rect().contains(world))
    }

    // ------------------------------------------------------------------
    // Dessin
    // ------------------------------------------------------------------

    fn draw_all(&self, frame: &mut Frame) {
        frame.translate(self.offset);
        frame.scale(self.zoom);

        for rel in &self.relations {
            let (Some(&a), Some(&b)) = (self.index.get(&rel.from), self.index.get(&rel.to)) else {
                continue;
            };
            self.draw_relation(frame, rel, a, b);
        }
        for (i, node) in self.nodes.iter().enumerate() {
            self.draw_node(frame, node, self.selected == Some(i));
        }
    }

    fn draw_node(&self, frame: &mut Frame, node: &Node, selected: bool) {
        let rect = node.rect();
        let association = self.mode == Mode::Conceptual && node.junction;
        let header_color = if association { theme::WARNING } else { theme::PRIMARY };
        let border = if selected { theme::TEXT } else { theme::BORDER };

        let body = Path::rounded_rectangle(rect.position(), rect.size(), 6.0.into());
        frame.fill(&body, theme::SURFACE);
        let header = Path::rectangle(rect.position() + Vector::new(1.0, 1.0), Size::new(rect.width - 2.0, HEADER_HEIGHT));
        frame.fill(&header, Color { a: 0.25, ..header_color });
        frame.stroke(&body, Stroke::default().with_color(border).with_width(if selected { 2.0 } else { 1.0 }));

        frame.fill_text(Text {
            content: node.table.name.clone(),
            position: rect.position() + Vector::new(PADDING, HEADER_HEIGHT / 2.0),
            color: header_color,
            size: (FONT_SIZE + 1.0).into(),
            font: FONT,
            align_y: alignment::Vertical::Center,
            ..Text::default()
        });

        for (i, line) in node.lines.iter().enumerate() {
            let y = rect.y + HEADER_HEIGHT + PADDING / 2.0 + i as f32 * ROW_HEIGHT + ROW_HEIGHT / 2.0;
            let color = if line.starts_with("PK") {
                theme::TEXT
            } else if line.starts_with("FK") {
                theme::MUTED
            } else {
                theme::TEXT
            };
            frame.fill_text(Text {
                content: line.clone(),
                position: Point::new(rect.x + PADDING, y),
                color,
                size: FONT_SIZE.into(),
                font: FONT,
                align_y: alignment::Vertical::Center,
                ..Text::default()
            });
        }
    }

    fn draw_relation(&self, frame: &mut Frame, rel: &Relation, from: usize, to: usize) {
        let (a, b) = (&self.nodes[from], &self.nodes[to]);
        if from == to {
            return;
        }
        let (start, end) = (edge_point(a.rect(), b.center()), edge_point(b.rect(), a.center()));
        let stroke = Stroke::default().with_color(theme::MUTED).with_width(1.5);
        frame.stroke(&Path::line(start, end), stroke);

        let dir = end - start;
        let len = (dir.x * dir.x + dir.y * dir.y).sqrt().max(1.0);
        let unit = dir * (1.0 / len);
        let normal = Vector::new(-unit.y, unit.x);

        match self.mode {
            Mode::Conceptual => {
                // Cardinalités : côté référençant 0,n ; côté référencé 1,1 ou 0,1.
                let nullable = rel.from_columns.iter().any(|c| {
                    a.columns.iter().any(|col| &col.name == c && col.nullable)
                });
                let near = |p: Point, t: f32| p + unit * t + normal * 10.0;
                self.label(frame, "0,n", near(start, 22.0));
                self.label(frame, if nullable { "0,1" } else { "1,1" }, near(end, -22.0));
            }
            _ => {
                // Patte d'oie côté « plusieurs », barre côté « un ».
                let foot = 12.0;
                let base = start + unit * foot;
                for side in [-1.0, 1.0] {
                    frame.stroke(&Path::line(start + normal * (6.0 * side), base), stroke);
                }
                frame.stroke(&Path::line(start, base), stroke);
                let bar = end - unit * 8.0;
                frame.stroke(&Path::line(bar + normal * 6.0, bar - normal * 6.0), stroke);
            }
        }
    }

    fn label(&self, frame: &mut Frame, content: &str, at: Point) {
        frame.fill_text(Text {
            content: content.into(),
            position: at,
            color: theme::WARNING,
            size: (FONT_SIZE - 1.0).into(),
            font: FONT,
            align_x: alignment::Horizontal::Center.into(),
            align_y: alignment::Vertical::Center,
            ..Text::default()
        });
    }
}

/// Lignes affichées dans une boîte selon le mode.
fn describe(mode: Mode, columns: &[ColumnInfo], fks: Option<&HashSet<String>>, junction: bool) -> Vec<String> {
    let is_fk = |c: &ColumnInfo| fks.is_some_and(|f| f.contains(&c.name));
    columns
        .iter()
        .filter_map(|c| {
            let tag = if c.primary_key { "PK" } else if is_fk(c) { "FK" } else { "  " };
            match mode {
                Mode::Physical => Some(format!("{tag} {} : {}{}", c.name, c.data_type, if c.nullable { "?" } else { "" })),
                Mode::Logical => Some(format!("{tag} {}", c.name)),
                Mode::Conceptual => {
                    // Les clés étrangères deviennent des associations : on les cache.
                    if is_fk(c) && (!c.primary_key || junction) {
                        None
                    } else {
                        Some(format!("{} {}", if c.primary_key { "#" } else { " " }, c.name))
                    }
                }
            }
        })
        .collect()
}

/// Table de jonction : sa clé primaire n'est faite que de clés étrangères
/// (ou elle n'a aucune colonne hors clés étrangères) et elle pointe vers
/// au moins deux tables.
fn is_junction(columns: &[ColumnInfo], fks: Option<&HashSet<String>>, outgoing: usize) -> bool {
    let Some(fks) = fks else { return false };
    if outgoing < 2 {
        return false;
    }
    let pk: Vec<_> = columns.iter().filter(|c| c.primary_key).collect();
    if pk.is_empty() {
        columns.iter().all(|c| fks.contains(&c.name))
    } else {
        pk.iter().all(|c| fks.contains(&c.name))
    }
}

/// Point du bord de `rect` sur la droite entre son centre et `target`.
fn edge_point(rect: Rectangle, target: Point) -> Point {
    let center = rect.center();
    let d = target - center;
    if d.x.abs() < 0.001 && d.y.abs() < 0.001 {
        return center;
    }
    let (hw, hh) = (rect.width / 2.0, rect.height / 2.0);
    let t = (hw / d.x.abs()).min(hh / d.y.abs());
    center + d * t
}

#[derive(Debug, Default)]
pub struct State {
    drag: Option<Drag>,
}

#[derive(Debug, Clone, Copy)]
enum Drag {
    Node { index: usize, grab: Vector },
    Pan { last: Point },
}

impl canvas::Program<Interaction> for Diagram {
    type State = State;

    fn update(
        &self,
        state: &mut State,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Interaction>> {
        let Event::Mouse(event) = event else { return None };
        let position = cursor.position_in(bounds);

        match event {
            mouse::Event::ButtonPressed(mouse::Button::Left) => {
                let screen = position?;
                let world = self.to_world(screen);
                state.drag = Some(match self.node_at(world) {
                    Some(index) => Drag::Node { index, grab: world - self.nodes[index].position },
                    None => Drag::Pan { last: screen },
                });
                let selected = match state.drag {
                    Some(Drag::Node { index, .. }) => Some(index),
                    _ => None,
                };
                Some(canvas::Action::publish(Interaction::Select(selected)).and_capture())
            }
            mouse::Event::CursorMoved { .. } => {
                let screen = position?;
                match state.drag? {
                    Drag::Node { index, grab } => {
                        let to = self.to_world(screen) - grab;
                        Some(canvas::Action::publish(Interaction::MoveNode { index, to }).and_capture())
                    }
                    Drag::Pan { last } => {
                        state.drag = Some(Drag::Pan { last: screen });
                        Some(canvas::Action::publish(Interaction::Pan(screen - last)).and_capture())
                    }
                }
            }
            mouse::Event::ButtonReleased(mouse::Button::Left) => {
                state.drag = None;
                Some(canvas::Action::request_redraw())
            }
            mouse::Event::WheelScrolled { delta } => {
                let at = position?;
                let lines = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => *y,
                    mouse::ScrollDelta::Pixels { y, .. } => *y / 40.0,
                };
                let factor = 1.1_f32.powf(lines);
                Some(canvas::Action::publish(Interaction::Zoom { factor, at }).and_capture())
            }
            _ => None,
        }
    }

    fn draw(
        &self,
        _state: &State,
        renderer: &iced::Renderer,
        _theme: &iced::Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        self.viewport.set(bounds.size());
        vec![self.cache.draw(renderer, bounds.size(), |frame| self.draw_all(frame))]
    }

    fn mouse_interaction(&self, state: &State, bounds: Rectangle, cursor: mouse::Cursor) -> mouse::Interaction {
        match state.drag {
            Some(Drag::Node { .. }) => mouse::Interaction::Grabbing,
            Some(Drag::Pan { .. }) => mouse::Interaction::Move,
            None => match cursor.position_in(bounds) {
                Some(p) if self.node_at(self.to_world(p)).is_some() => mouse::Interaction::Grab,
                _ => mouse::Interaction::default(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(name: &str, pk: bool) -> ColumnInfo {
        ColumnInfo { name: name.into(), data_type: "int".into(), nullable: false, default: None, primary_key: pk }
    }

    #[test]
    fn junction_table_is_detected() {
        let fks: HashSet<String> = ["user_id", "badge_id"].map(String::from).into();
        let cols = [col("user_id", true), col("badge_id", true), col("granted_at", false)];
        assert!(is_junction(&cols, Some(&fks), 2));
        // Une clé propre en fait une entité, pas une association.
        let cols = [col("id", true), col("user_id", false), col("badge_id", false)];
        assert!(!is_junction(&cols, Some(&fks), 2));
    }

    #[test]
    fn edge_point_lies_on_border() {
        let rect = Rectangle::new(Point::new(0.0, 0.0), Size::new(100.0, 50.0));
        let p = edge_point(rect, Point::new(300.0, 25.0));
        assert!((p.x - 100.0).abs() < 0.01 && (p.y - 25.0).abs() < 0.01);
    }
}
