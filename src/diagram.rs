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

use crate::connector::{ColumnDef, ColumnInfo, Connector, Relation, TableRef};
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
    /// Poignée relâchée sur une autre table (mode conception).
    Link { from: usize, to: usize },
}

/// Sens d'une relation créée en glisser-déposer, de `from` vers `to`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    /// Un `from` a plusieurs `to` : la clé étrangère va dans `to`.
    OneToMany,
    /// Plusieurs `from` pour un `to` : la clé étrangère va dans `from`.
    ManyToOne,
    /// Table de jonction entre les deux.
    ManyToMany,
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
    /// Table déjà présente dans la base (pas dans le script généré).
    pub existing: bool,
}

const HANDLE_RADIUS: f32 = 6.0;

impl Node {
    fn rect(&self) -> Rectangle {
        Rectangle::new(self.position, self.size)
    }

    /// Poignée de liaison, au bord droit de l'en-tête.
    fn handle(&self) -> Point {
        Point::new(self.position.x + self.size.width, self.position.y + HEADER_HEIGHT / 2.0)
    }

    fn center(&self) -> Point {
        self.position + Vector::new(self.size.width / 2.0, self.size.height / 2.0)
    }
}

pub struct Diagram {
    pub nodes: Vec<Node>,
    pub relations: Vec<Relation>,
    pub mode: Mode,
    /// Mode conception : tables et relations modifiables, poignées de liaison.
    pub editable: bool,
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
                Node { table, columns, position: Point::ORIGIN, size: Size::ZERO, lines: Vec::new(), junction, existing: true }
            })
            .collect();

        let mut diagram = Self {
            nodes,
            relations,
            mode: Mode::Logical,
            editable: false,
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

    /// Diagramme vide, prêt à être dessiné.
    pub fn empty() -> Self {
        Self::new(Vec::new(), Vec::new())
    }

    /// Recalcule index, clés étrangères, jonctions et tailles après une
    /// modification de structure.
    fn refresh(&mut self) {
        self.index = self.nodes.iter().enumerate().map(|(i, n)| (n.table.clone(), i)).collect();
        self.fk_columns.clear();
        for rel in &self.relations {
            self.fk_columns.entry(rel.from.clone()).or_default().extend(rel.from_columns.iter().cloned());
        }
        for node in &mut self.nodes {
            let outgoing = self.relations.iter().filter(|r| r.from == node.table).count();
            node.junction = is_junction(&node.columns, self.fk_columns.get(&node.table), outgoing);
        }
        self.set_mode(self.mode);
    }

    // ------------------------------------------------------------------
    // Mode conception
    // ------------------------------------------------------------------

    /// Ajoute une table vide (avec un `id` clé primaire) au centre de la vue
    /// et la sélectionne. Renvoie son indice.
    pub fn add_table(&mut self, schema: Option<String>, id_type: &str) -> usize {
        let base = "nouvelle_table";
        let mut name = base.to_string();
        let mut n = 1;
        while self.nodes.iter().any(|node| node.table.name == name) {
            n += 1;
            name = format!("{base}_{n}");
        }
        let view = self.viewport.get();
        let center = self.to_world(Point::new(view.width / 2.0, view.height / 2.0));
        let offset = self.nodes.len() as f32 * 24.0 % 160.0;
        self.nodes.push(Node {
            table: TableRef { schema, name },
            columns: vec![ColumnInfo {
                name: "id".into(),
                data_type: id_type.into(),
                nullable: false,
                default: None,
                primary_key: true,
            }],
            position: Point::new(center.x - 80.0 + offset, center.y - 40.0 + offset),
            size: Size::ZERO,
            lines: Vec::new(),
            junction: false,
            existing: false,
        });
        self.selected = Some(self.nodes.len() - 1);
        self.refresh();
        self.nodes.len() - 1
    }

    pub fn remove_table(&mut self, index: usize) {
        if index >= self.nodes.len() {
            return;
        }
        let table = self.nodes.remove(index).table;
        self.relations.retain(|r| r.from != table && r.to != table);
        self.selected = None;
        self.refresh();
    }

    pub fn rename_table(&mut self, index: usize, name: &str) {
        let Some(node) = self.nodes.get_mut(index) else { return };
        let old = node.table.clone();
        node.table.name = name.to_string();
        let new = node.table.clone();
        for rel in &mut self.relations {
            if rel.from == old {
                rel.from = new.clone();
            }
            if rel.to == old {
                rel.to = new.clone();
            }
        }
        self.refresh();
    }

    /// Modifie une colonne ; `f` reçoit la colonne. Les relations suivent
    /// un renommage.
    pub fn edit_column(&mut self, index: usize, column: usize, f: impl FnOnce(&mut ColumnInfo)) {
        let Some(node) = self.nodes.get_mut(index) else { return };
        let Some(col) = node.columns.get_mut(column) else { return };
        let old = col.name.clone();
        f(col);
        let new = col.name.clone();
        if old != new {
            let table = node.table.clone();
            for rel in &mut self.relations {
                if rel.from == table {
                    for c in &mut rel.from_columns {
                        if *c == old {
                            *c = new.clone();
                        }
                    }
                }
                if rel.to == table {
                    for c in &mut rel.to_columns {
                        if *c == old {
                            *c = new.clone();
                        }
                    }
                }
            }
        }
        self.refresh();
    }

    pub fn add_column(&mut self, index: usize) {
        let Some(node) = self.nodes.get_mut(index) else { return };
        node.columns.push(ColumnInfo {
            name: String::new(),
            data_type: String::new(),
            nullable: true,
            default: None,
            primary_key: false,
        });
        self.refresh();
    }

    pub fn remove_column(&mut self, index: usize, column: usize) {
        let Some(node) = self.nodes.get_mut(index) else { return };
        if column >= node.columns.len() {
            return;
        }
        let name = node.columns.remove(column).name;
        let table = node.table.clone();
        self.relations.retain(|r| {
            !(r.from == table && r.from_columns.contains(&name)) && !(r.to == table && r.to_columns.contains(&name))
        });
        self.refresh();
    }

    pub fn remove_relation(&mut self, index: usize) {
        if index < self.relations.len() {
            self.relations.remove(index);
            self.refresh();
        }
    }

    /// Crée une relation entre deux tables en ajoutant ce qu'il faut : la
    /// clé étrangère côté « plusieurs », ou une table de jonction.
    pub fn link(&mut self, from: usize, to: usize, kind: LinkKind) -> Result<(), String> {
        if from == to {
            return Err("Une table ne peut pas être liée à elle-même ici".into());
        }
        let pk_of = |node: &Node| -> Result<(String, String), String> {
            node.columns
                .iter()
                .find(|c| c.primary_key)
                .map(|c| (c.name.clone(), c.data_type.clone()))
                .ok_or_else(|| format!("La table {} n'a pas de clé primaire", node.table.name))
        };

        match kind {
            LinkKind::OneToMany | LinkKind::ManyToOne => {
                // La clé étrangère va dans la table « plusieurs ».
                let (one, many) = if kind == LinkKind::OneToMany { (from, to) } else { (to, from) };
                let (pk_name, pk_type) = pk_of(&self.nodes[one])?;
                let fk_name = format!("{}_{pk_name}", singular(&self.nodes[one].table.name));
                let fk_type = reference_type(&pk_type);
                let many_table = self.nodes[many].table.clone();
                let one_table = self.nodes[one].table.clone();
                if !self.nodes[many].columns.iter().any(|c| c.name == fk_name) {
                    self.nodes[many].columns.push(ColumnInfo {
                        name: fk_name.clone(),
                        data_type: fk_type,
                        nullable: false,
                        default: None,
                        primary_key: false,
                    });
                }
                self.relations.push(Relation {
                    from: many_table,
                    from_columns: vec![fk_name],
                    to: one_table,
                    to_columns: vec![pk_name],
                });
            }
            LinkKind::ManyToMany => {
                let (a_pk, a_type) = pk_of(&self.nodes[from])?;
                let (b_pk, b_type) = pk_of(&self.nodes[to])?;
                let (a, b) = (self.nodes[from].table.clone(), self.nodes[to].table.clone());
                let a_fk = format!("{}_{a_pk}", singular(&a.name));
                let b_fk = format!("{}_{b_pk}", singular(&b.name));
                let junction = TableRef { schema: a.schema.clone(), name: format!("{}_{}", singular(&a.name), b.name) };
                let mid = Point::new(
                    (self.nodes[from].position.x + self.nodes[to].position.x) / 2.0,
                    (self.nodes[from].position.y + self.nodes[to].position.y) / 2.0 + 120.0,
                );
                let fk = |name: String, data_type: String| ColumnInfo {
                    name,
                    data_type: reference_type(&data_type),
                    nullable: false,
                    default: None,
                    primary_key: true,
                };
                self.nodes.push(Node {
                    table: junction.clone(),
                    columns: vec![fk(a_fk.clone(), a_type), fk(b_fk.clone(), b_type)],
                    position: mid,
                    size: Size::ZERO,
                    lines: Vec::new(),
                    junction: true,
                    existing: false,
                });
                self.relations.push(Relation { from: junction.clone(), from_columns: vec![a_fk], to: a, to_columns: vec![a_pk] });
                self.relations.push(Relation { from: junction, from_columns: vec![b_fk], to: b, to_columns: vec![b_pk] });
            }
        }
        self.refresh();
        Ok(())
    }

    /// Script de création des tables **nouvelles** (pas celles déjà en
    /// base), dans un ordre qui respecte les clés étrangères.
    pub fn to_ddl(&self, c: &dyn Connector) -> Result<String, String> {
        let new: Vec<usize> = (0..self.nodes.len()).filter(|&i| !self.nodes[i].existing).collect();
        if new.is_empty() {
            return Err("Aucune nouvelle table : ajoute des tables au diagramme".into());
        }
        for &i in &new {
            let node = &self.nodes[i];
            if node.table.name.trim().is_empty() {
                return Err("Une table n'a pas de nom".into());
            }
            for col in &node.columns {
                if col.name.trim().is_empty() || col.data_type.trim().is_empty() {
                    return Err(format!("La table {} a une colonne sans nom ou sans type", node.table.name));
                }
            }
        }

        // Tri topologique : une table après celles qu'elle référence.
        let mut order: Vec<usize> = Vec::new();
        let mut remaining: Vec<usize> = new.clone();
        while !remaining.is_empty() {
            let before = remaining.len();
            remaining.retain(|&i| {
                let table = &self.nodes[i].table;
                let blocked = self.relations.iter().any(|r| {
                    r.from == *table
                        && r.to != *table
                        && self.index.get(&r.to).is_some_and(|&j| !self.nodes[j].existing && !order.contains(&j))
                });
                if blocked { true } else { order.push(i); false }
            });
            if remaining.len() == before {
                // Cycle : on prend ce qui reste tel quel.
                order.extend(remaining.drain(..));
            }
        }

        let mut statements = Vec::new();
        for i in order {
            let node = &self.nodes[i];
            let columns: Vec<ColumnDef> = node
                .columns
                .iter()
                .map(|col| ColumnDef {
                    name: col.name.trim().into(),
                    data_type: col.data_type.trim().into(),
                    nullable: col.nullable,
                    primary_key: col.primary_key,
                })
                .collect();
            let mut sql = c.create_table_sql(&node.table, &columns);
            let fks: Vec<String> = self
                .relations
                .iter()
                .filter(|r| r.from == node.table)
                .map(|r| {
                    format!(
                        "    FOREIGN KEY ({}) REFERENCES {} ({})",
                        r.from_columns.iter().map(|col| c.quote_identifier(col)).collect::<Vec<_>>().join(", "),
                        c.quote_table(&r.to),
                        r.to_columns.iter().map(|col| c.quote_identifier(col)).collect::<Vec<_>>().join(", ")
                    )
                })
                .collect();
            if !fks.is_empty() {
                // Insère les contraintes avant la parenthèse fermante.
                let end = sql.rfind("
)").unwrap_or(sql.len());
                sql.insert_str(end, &format!(",
{}", fks.join(",
")));
            }
            statements.push(sql + ";");
        }
        Ok(statements.join("

"))
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
            // La liaison demande un choix (1-n, n-1, n-n) : l'application s'en charge.
            Interaction::Link { .. } => {}
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

    fn handle_at(&self, world: Point) -> Option<usize> {
        self.nodes.iter().rposition(|n| {
            let h = n.handle();
            let d = world - h;
            (d.x * d.x + d.y * d.y).sqrt() <= HANDLE_RADIUS * 2.0 / self.zoom.max(0.5)
        })
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

        if self.editable {
            let handle = Path::circle(node.handle(), HANDLE_RADIUS);
            frame.fill(&handle, if node.existing { theme::MUTED } else { theme::SUCCESS });
            frame.stroke(&handle, Stroke::default().with_color(theme::BACKGROUND).with_width(1.5));
        }

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
    /// Trait élastique depuis la poignée de `from` jusqu'au curseur (écran).
    Link { from: usize, cursor: Point },
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
                if self.editable
                    && let Some(from) = self.handle_at(world)
                {
                    state.drag = Some(Drag::Link { from, cursor: screen });
                    return Some(canvas::Action::publish(Interaction::Select(Some(from))).and_capture());
                }
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
                    Drag::Link { from, .. } => {
                        state.drag = Some(Drag::Link { from, cursor: screen });
                        Some(canvas::Action::request_redraw().and_capture())
                    }
                }
            }
            mouse::Event::ButtonReleased(mouse::Button::Left) => {
                let drag = state.drag.take();
                if let Some(Drag::Link { from, .. }) = drag
                    && let Some(screen) = position
                    && let Some(to) = self.node_at(self.to_world(screen))
                    && to != from
                {
                    return Some(canvas::Action::publish(Interaction::Link { from, to }).and_capture());
                }
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
        state: &State,
        renderer: &iced::Renderer,
        _theme: &iced::Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        self.viewport.set(bounds.size());
        let mut layers = vec![self.cache.draw(renderer, bounds.size(), |frame| self.draw_all(frame))];
        // Trait élastique pendant une liaison : hors cache, il bouge à chaque image.
        if let Some(Drag::Link { from, cursor }) = state.drag
            && let Some(node) = self.nodes.get(from)
        {
            let mut frame = Frame::new(renderer, bounds.size());
            let start = Point::new(
                node.handle().x * self.zoom + self.offset.x,
                node.handle().y * self.zoom + self.offset.y,
            );
            frame.stroke(
                &Path::line(start, cursor),
                Stroke::default().with_color(theme::SUCCESS).with_width(2.0),
            );
            frame.fill(&Path::circle(cursor, 4.0), theme::SUCCESS);
            layers.push(frame.into_geometry());
        }
        layers
    }

    fn mouse_interaction(&self, state: &State, bounds: Rectangle, cursor: mouse::Cursor) -> mouse::Interaction {
        match state.drag {
            Some(Drag::Node { .. }) => mouse::Interaction::Grabbing,
            Some(Drag::Pan { .. }) => mouse::Interaction::Move,
            Some(Drag::Link { .. }) => mouse::Interaction::Crosshair,
            None => match cursor.position_in(bounds) {
                Some(p) if self.editable && self.handle_at(self.to_world(p)).is_some() => mouse::Interaction::Crosshair,
                Some(p) if self.node_at(self.to_world(p)).is_some() => mouse::Interaction::Grab,
                _ => mouse::Interaction::default(),
            },
        }
    }
}

/// `users` → `user`, `categories` → `category` : pour nommer `user_id`.
fn singular(name: &str) -> String {
    if let Some(stem) = name.strip_suffix("ies") {
        format!("{stem}y")
    } else if name.ends_with("ss") {
        name.to_string()
    } else if let Some(stem) = name.strip_suffix('s') {
        stem.to_string()
    } else {
        name.to_string()
    }
}

/// Type d'une clé étrangère d'après celui de la clé référencée : un
/// auto-incrément devient l'entier correspondant.
fn reference_type(pk_type: &str) -> String {
    match pk_type.to_uppercase().as_str() {
        "SERIAL" => "INTEGER".into(),
        "BIGSERIAL" => "BIGINT".into(),
        "SMALLSERIAL" => "SMALLINT".into(),
        _ => pk_type.to_string(),
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
    fn singular_forms() {
        assert_eq!(singular("users"), "user");
        assert_eq!(singular("categories"), "category");
        assert_eq!(singular("courses"), "course");
        assert_eq!(singular("address"), "address");
    }

    #[test]
    fn one_to_many_adds_foreign_key() {
        let mut d = Diagram::empty();
        let users = d.add_table(None, "SERIAL");
        d.rename_table(users, "users");
        let orders = d.add_table(None, "SERIAL");
        d.rename_table(orders, "orders");
        d.link(users, orders, LinkKind::OneToMany).unwrap();

        let fk = d.nodes[orders].columns.iter().find(|c| c.name == "user_id").expect("user_id ajouté");
        assert_eq!(fk.data_type, "INTEGER");
        assert_eq!(d.relations.len(), 1);
        assert_eq!(d.relations[0].from.name, "orders");
        assert_eq!(d.relations[0].to.name, "users");
    }

    #[test]
    fn many_to_many_creates_junction() {
        let mut d = Diagram::empty();
        let a = d.add_table(None, "INTEGER");
        d.rename_table(a, "students");
        let b = d.add_table(None, "INTEGER");
        d.rename_table(b, "courses");
        d.link(a, b, LinkKind::ManyToMany).unwrap();
        assert_eq!(d.nodes.len(), 3);
        let j = &d.nodes[2];
        assert_eq!(j.table.name, "student_courses");
        assert!(j.junction);
        assert_eq!(d.relations.len(), 2);
    }

    #[test]
    fn edge_point_lies_on_border() {
        let rect = Rectangle::new(Point::new(0.0, 0.0), Size::new(100.0, 50.0));
        let p = edge_point(rect, Point::new(300.0, 25.0));
        assert!((p.x - 100.0).abs() < 0.01 && (p.y - 25.0).abs() < 0.01);
    }
}
