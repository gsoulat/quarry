//! Plans d'exécution : chaque moteur a son propre format d'`EXPLAIN`, que l'on
//! ramène à une liste plate de nœuds.

use std::collections::HashMap;

use serde_json::Value;

/// Une étape du plan. Les nœuds sont stockés en **pré-ordre**, comme les
/// affiche `EXPLAIN` : un parent apparaît avant ses enfants, et `depth`
/// indique le niveau d'imbrication.
#[derive(Debug, Clone)]
pub struct PlanNode {
    pub depth: usize,
    pub label: String,
    /// Coût cumulé estimé de ce nœud (et de ses enfants).
    pub cost: Option<f64>,
    /// Nombre de lignes estimé en sortie de ce nœud.
    pub rows: Option<f64>,
}

#[derive(Debug, Clone, Default)]
pub struct QueryPlan {
    pub nodes: Vec<PlanNode>,
    pub total_cost: Option<f64>,
    /// Unité du coût : chaque moteur a son échelle, non comparable aux autres.
    pub cost_unit: &'static str,
}

/// Renvoie les indices de `nodes` dans l'ordre où le moteur les exécute.
///
/// Un nœud a besoin du résultat de ses enfants : les enfants passent donc
/// avant leur parent, et des frères s'exécutent dans l'ordre d'affichage.
/// Les enfants d'un nœud de profondeur `d` sont les nœuds suivants de
/// profondeur `d + 1`, jusqu'au prochain nœud de profondeur `<= d`.
/// Il peut y avoir plusieurs racines (SQLite renvoie parfois une forêt).
pub fn execution_order(nodes: &[PlanNode]) -> Vec<usize> {
    // TODO(toi) : implémenter le parcours. Cette version provisoire inverse
    // simplement la liste, ce qui est faux dès qu'un nœud a plusieurs enfants.
    (0..nodes.len()).rev().collect()
}

/// `EXPLAIN (FORMAT JSON)` de PostgreSQL.
pub fn from_postgres_json(value: &Value) -> Result<QueryPlan, String> {
    let root = value
        .get(0)
        .and_then(|v| v.get("Plan"))
        .ok_or("Plan PostgreSQL illisible")?;

    let mut nodes = Vec::new();
    walk_postgres(root, 0, &mut nodes);

    Ok(QueryPlan {
        total_cost: root.get("Total Cost").and_then(Value::as_f64),
        nodes,
        cost_unit: "unités PostgreSQL",
    })
}

fn walk_postgres(node: &Value, depth: usize, out: &mut Vec<PlanNode>) {
    let field = |key: &str| node.get(key).and_then(Value::as_str);

    let mut label = field("Node Type").unwrap_or("?").to_string();
    if let Some(join) = field("Join Type") {
        label = format!("{label} ({join})");
    }
    if let Some(relation) = field("Relation Name") {
        label.push_str(&format!(" sur {relation}"));
    }
    if let Some(index) = field("Index Name") {
        label.push_str(&format!(" via {index}"));
    }

    out.push(PlanNode {
        depth,
        label,
        cost: node.get("Total Cost").and_then(Value::as_f64),
        rows: node.get("Plan Rows").and_then(Value::as_f64),
    });

    for child in node.get("Plans").and_then(Value::as_array).into_iter().flatten() {
        walk_postgres(child, depth + 1, out);
    }
}

/// `EXPLAIN FORMAT=TREE` de MySQL (8.0.18+), par exemple :
/// `-> Filter: (t.id > 3)  (cost=1.2 rows=3)`
pub fn from_mysql_tree(text: &str) -> QueryPlan {
    let nodes: Vec<PlanNode> = text
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim_start();
            let content = trimmed.strip_prefix("-> ")?;
            let (label, stats) = match content.rfind("  (") {
                Some(i) => (&content[..i], &content[i..]),
                None => (content, ""),
            };
            Some(PlanNode {
                depth: (line.len() - trimmed.len()) / 4,
                label: label.to_string(),
                cost: number_after(stats, "cost="),
                rows: number_after(stats, "rows="),
            })
        })
        .collect();

    QueryPlan {
        total_cost: nodes.first().and_then(|n| n.cost),
        nodes,
        cost_unit: "unités MySQL",
    }
}

/// `EXPLAIN QUERY PLAN` de SQLite : lignes `(id, parent, detail)`.
/// SQLite ne fournit pas de coût.
pub fn from_sqlite(rows: &[(i64, i64, String)]) -> QueryPlan {
    let mut depth_of: HashMap<i64, usize> = HashMap::new();
    let nodes = rows
        .iter()
        .map(|(id, parent, detail)| {
            let depth = depth_of.get(parent).map_or(0, |d| d + 1);
            depth_of.insert(*id, depth);
            PlanNode { depth, label: detail.clone(), cost: None, rows: None }
        })
        .collect();

    QueryPlan { nodes, total_cost: None, cost_unit: "" }
}

/// Lit le nombre qui suit `key` ; pour une plage `a..b`, garde `b`.
fn number_after(text: &str, key: &str) -> Option<f64> {
    let rest = &text[text.find(key)? + key.len()..];
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || matches!(c, '.' | 'e' | '+' | '-')))
        .unwrap_or(rest.len());
    rest[..end].rsplit("..").next()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(depth: usize, label: &str) -> PlanNode {
        PlanNode { depth, label: label.into(), cost: None, rows: None }
    }

    #[test]
    fn hash_join_runs_children_first() {
        // Hash Join
        //   Seq Scan orders
        //   Hash
        //     Seq Scan users
        let nodes = [
            node(0, "Hash Join"),
            node(1, "Seq Scan orders"),
            node(1, "Hash"),
            node(2, "Seq Scan users"),
        ];
        assert_eq!(execution_order(&nodes), vec![1, 3, 2, 0]);
    }

    #[test]
    fn forest_keeps_sibling_order() {
        let nodes = [node(0, "SCAN a"), node(0, "SCAN b")];
        assert_eq!(execution_order(&nodes), vec![0, 1]);
    }

    #[test]
    fn mysql_tree_is_parsed() {
        let plan = from_mysql_tree(
            "-> Filter: (t.id > 3)  (cost=1.25 rows=3)\n    -> Table scan on t  (cost=0.75 rows=10)\n",
        );
        assert_eq!(plan.nodes.len(), 2);
        assert_eq!(plan.nodes[1].depth, 1);
        assert_eq!(plan.nodes[1].label, "Table scan on t");
        assert_eq!(plan.total_cost, Some(1.25));
    }

    #[test]
    fn cost_range_keeps_upper_bound() {
        assert_eq!(number_after("(cost=0.25..0.35 rows=1)", "cost="), Some(0.35));
    }
}
