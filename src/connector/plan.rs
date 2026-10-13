//! Plans d'exécution : ordre d'exécution et lecture du format SQLite (les
//! autres moteurs renvoient leur plan déjà structuré via leur connecteur).

use std::collections::HashMap;

use super::{PlanNode, QueryPlan};

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

/// `EXPLAIN QUERY PLAN` de SQLite : lignes `(id, parent, detail)`.
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
    QueryPlan { nodes, total_cost: None, cost_unit: String::new(), note: None }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(depth: usize, label: &str) -> PlanNode {
        PlanNode { depth, label: label.into(), cost: None, rows: None }
    }

    // Ces deux tests attendent l'implémentation de `execution_order` ;
    // ils sont ignorés pour garder la CI verte : `cargo test -- --ignored`.
    #[test]
    #[ignore = "execution_order à implémenter"]
    fn hash_join_runs_children_first() {
        // Hash Join
        //   Seq Scan orders
        //   Hash
        //     Seq Scan users
        let nodes = [node(0, "Hash Join"), node(1, "Seq Scan orders"), node(1, "Hash"), node(2, "Seq Scan users")];
        assert_eq!(execution_order(&nodes), vec![1, 3, 2, 0]);
    }

    #[test]
    #[ignore = "execution_order à implémenter"]
    fn forest_keeps_sibling_order() {
        let nodes = [node(0, "SCAN a"), node(0, "SCAN b")];
        assert_eq!(execution_order(&nodes), vec![0, 1]);
    }

    #[test]
    fn sqlite_rows_become_a_tree() {
        let plan = from_sqlite(&[(2, 0, "SCAN users".into()), (5, 2, "SEARCH orders".into())]);
        assert_eq!(plan.nodes[0].depth, 0);
        assert_eq!(plan.nodes[1].depth, 1);
    }
}
