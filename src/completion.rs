//! Autocomplétion de l'éditeur SQL : mots-clés, tables et colonnes.

use std::collections::HashMap;

use crate::connector::{ColumnInfo, TableRef};

pub const KEYWORDS: &[&str] = &[
    "SELECT", "FROM", "WHERE", "JOIN", "LEFT JOIN", "INNER JOIN", "RIGHT JOIN", "ON", "GROUP BY",
    "ORDER BY", "HAVING", "LIMIT", "OFFSET", "INSERT INTO", "VALUES", "UPDATE", "SET", "DELETE FROM",
    "CREATE TABLE", "ALTER TABLE", "DROP TABLE", "AND", "OR", "NOT", "NULL", "IS NULL",
    "IS NOT NULL", "AS", "DISTINCT", "UNION", "COUNT(", "SUM(", "AVG(", "MIN(", "MAX(", "BETWEEN",
    "LIKE", "IN", "CASE", "WHEN", "THEN", "ELSE", "END", "ASC", "DESC", "EXISTS", "WITH",
    "RETURNING", "TRUE", "FALSE",
];

pub const MAX_ITEMS: usize = 8;

/// Liste proposée sous le curseur.
#[derive(Debug, Clone)]
pub struct Completion {
    pub items: Vec<String>,
    pub selected: usize,
    /// Caractères déjà tapés à remplacer par l'élément choisi.
    pub prefix_len: usize,
    /// Position du curseur (ligne, colonne) pour placer la liste.
    pub line: usize,
    pub column: usize,
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Mot en cours de frappe à la fin de `before` (texte avant le curseur) et,
/// s'il est précédé d'un point, le qualificatif qui le précède.
fn word_at_end(before: &str) -> (String, Option<String>) {
    let word: String = before.chars().rev().take_while(|&c| is_ident(c)).collect::<Vec<_>>().into_iter().rev().collect();
    let rest = &before[..before.len() - word.len()];
    let qualifier = rest.strip_suffix('.').map(|r| {
        r.chars().rev().take_while(|&c| is_ident(c)).collect::<Vec<_>>().into_iter().rev().collect::<String>()
    });
    (word, qualifier.filter(|q| !q.is_empty()))
}

/// Alias déclarés dans le texte (`FROM users u`, `JOIN orders AS o`) → table.
fn aliases(text: &str) -> HashMap<String, String> {
    let words: Vec<&str> = text.split(|c: char| c.is_whitespace() || c == ',').filter(|w| !w.is_empty()).collect();
    let mut map = HashMap::new();
    for (i, w) in words.iter().enumerate() {
        if matches!(w.to_uppercase().as_str(), "FROM" | "JOIN" | "UPDATE" | "INTO") {
            let Some(table) = words.get(i + 1) else { continue };
            let table = table.trim_matches('"').trim_matches('`');
            let mut j = i + 2;
            if words.get(j).is_some_and(|a| a.eq_ignore_ascii_case("AS")) {
                j += 1;
            }
            if let Some(alias) = words.get(j)
                && alias.chars().all(is_ident)
                && !KEYWORDS.iter().any(|k| k.eq_ignore_ascii_case(alias))
            {
                map.insert(alias.to_lowercase(), table.rsplit('.').next().unwrap_or(table).to_lowercase());
            }
        }
    }
    map
}

/// Calcule les propositions pour le texte avant le curseur.
pub fn suggest(
    text_before: &str,
    full_text: &str,
    line: usize,
    column: usize,
    tables: &[TableRef],
    columns: &HashMap<TableRef, Vec<ColumnInfo>>,
) -> Option<Completion> {
    let (word, qualifier) = word_at_end(text_before);
    if word.is_empty() && qualifier.is_none() {
        return None;
    }
    let needle = word.to_lowercase();
    let mut items: Vec<String> = Vec::new();

    match qualifier {
        // `table.` ou `alias.` : colonnes de cette table.
        Some(q) => {
            let q = q.to_lowercase();
            let alias_map = aliases(full_text);
            let target = alias_map.get(&q).cloned().unwrap_or(q.clone());
            for (table, cols) in columns {
                if table.name.to_lowercase() == target || table.schema.as_deref().is_some_and(|s| s.to_lowercase() == q) {
                    if table.schema.as_deref().is_some_and(|s| s.to_lowercase() == q) && table.name.to_lowercase() != target {
                        // `schema.` : propose les tables du schéma.
                        items.push(table.name.clone());
                        continue;
                    }
                    items.extend(cols.iter().map(|c| c.name.clone()));
                }
            }
            if items.is_empty() {
                items.extend(tables.iter().filter(|t| t.schema.as_deref().is_some_and(|s| s.to_lowercase() == q)).map(|t| t.name.clone()));
            }
        }
        None => {
            items.extend(tables.iter().map(|t| t.name.clone()));
            for cols in columns.values() {
                items.extend(cols.iter().map(|c| c.name.clone()));
            }
            items.extend(KEYWORDS.iter().map(|k| k.to_string()));
        }
    }

    let mut seen = std::collections::HashSet::new();
    let mut matched: Vec<String> = items
        .into_iter()
        .filter(|i| i.to_lowercase().starts_with(&needle) && i.to_lowercase() != needle)
        .filter(|i| seen.insert(i.to_lowercase()))
        .collect();
    // Les plus courts d'abord : « id » avant « identifiant ».
    matched.sort_by_key(|i| (i.len(), i.to_lowercase()));
    matched.truncate(MAX_ITEMS);
    if matched.is_empty() {
        return None;
    }
    Some(Completion { items: matched, selected: 0, prefix_len: word.chars().count(), line, column })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(name: &str) -> TableRef {
        TableRef { schema: Some("public".into()), name: name.into() }
    }
    fn col(name: &str) -> ColumnInfo {
        ColumnInfo { name: name.into(), data_type: "text".into(), nullable: true, default: None, primary_key: false }
    }
    fn fixture() -> (Vec<TableRef>, HashMap<TableRef, Vec<ColumnInfo>>) {
        let tables = vec![table("users"), table("user_badges")];
        let mut columns = HashMap::new();
        columns.insert(table("users"), vec![col("id"), col("email")]);
        columns.insert(table("user_badges"), vec![col("user_id"), col("badge_id")]);
        (tables, columns)
    }

    #[test]
    fn prefix_matches_tables_columns_and_keywords() {
        let (t, c) = fixture();
        let s = suggest("SELECT * FROM us", "SELECT * FROM us", 0, 16, &t, &c).unwrap();
        assert_eq!(s.prefix_len, 2);
        assert_eq!(s.items, vec!["users", "user_id", "user_badges"]);
        let s = suggest("sel", "sel", 0, 3, &t, &c).unwrap();
        assert_eq!(s.items, vec!["SELECT"]);
        assert!(suggest("SELECT ", "SELECT ", 0, 7, &t, &c).is_none());
    }

    #[test]
    fn dot_completes_columns_through_alias() {
        let (t, c) = fixture();
        let text = "SELECT u. FROM users u";
        let s = suggest("SELECT u.", text, 0, 9, &t, &c).unwrap();
        assert_eq!(s.items, vec!["id", "email"]);
        assert_eq!(s.prefix_len, 0);
        let s = suggest("SELECT users.e", "SELECT users.e", 0, 14, &t, &c).unwrap();
        assert_eq!(s.items, vec!["email"]);
    }

    #[test]
    fn schema_dot_lists_tables() {
        let (t, c) = fixture();
        let s = suggest("SELECT * FROM public.", "SELECT * FROM public.", 0, 21, &t, &c).unwrap();
        assert!(s.items.contains(&"users".to_string()));
    }
}
