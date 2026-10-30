use std::collections::{HashMap, HashSet};

use assetstudio_core::{AssetKey, SceneNode};

/// Flat DFS rows stay virtualized; checkboxes and collapsed branches refer to
/// stable row indexes so scrolling or hiding a branch never changes selection.
#[derive(Default)]
pub struct SceneTree {
    pub rows: Vec<(usize, usize)>,
    pub visible: Vec<usize>,
    ends: Vec<usize>,
    parents: Vec<Option<usize>>,
    by_key: HashMap<AssetKey, usize>,
    checked: Vec<bool>,
    collapsed: HashSet<usize>,
    pub checked_count: usize,
}

impl SceneTree {
    pub fn new(nodes: &[SceneNode], rows: Vec<(usize, usize)>) -> Self {
        let mut tree = Self {
            ends: vec![rows.len(); rows.len()],
            parents: vec![None; rows.len()],
            checked: vec![false; rows.len()],
            visible: (0..rows.len()).collect(),
            by_key: HashMap::with_capacity(rows.len()),
            rows,
            ..Default::default()
        };
        let mut stack: Vec<usize> = Vec::new();
        for (row, &(index, depth)) in tree.rows.iter().enumerate() {
            while stack
                .last()
                .is_some_and(|&parent| tree.rows[parent].1 >= depth)
            {
                tree.ends[stack.pop().unwrap()] = row;
            }
            tree.parents[row] = stack.last().copied();
            tree.by_key.insert(nodes[index].key, row);
            stack.push(row);
        }
        tree
    }

    pub fn is_checked(&self, row: usize) -> bool {
        self.checked[row]
    }
    pub fn has_children(&self, row: usize) -> bool {
        self.ends[row] > row + 1
    }
    pub fn is_collapsed(&self, row: usize) -> bool {
        self.collapsed.contains(&row)
    }

    pub fn set_checked(&mut self, row: usize, value: bool) {
        for checked in &mut self.checked[row..self.ends[row]] {
            if *checked != value {
                if value {
                    self.checked_count += 1;
                } else {
                    self.checked_count -= 1;
                }
                *checked = value;
            }
        }
        // An unchecked child must not be silently included through a checked
        // ancestor during split export. Other checked branches stay selected.
        if !value {
            let mut parent = self.parents[row];
            while let Some(row) = parent {
                if self.checked[row] {
                    self.checked[row] = false;
                    self.checked_count -= 1;
                }
                parent = self.parents[row];
            }
        }
    }

    pub fn check_all(&mut self, value: bool) {
        self.checked.fill(value);
        self.checked_count = if value { self.checked.len() } else { 0 };
    }

    pub fn checked_roots(&self, nodes: &[SceneNode]) -> Vec<AssetKey> {
        let mut roots = Vec::new();
        let mut row = 0;
        while row < self.rows.len() {
            if self.checked[row] {
                roots.push(nodes[self.rows[row].0].key);
                row = self.ends[row];
            } else {
                row += 1;
            }
        }
        roots
    }

    pub fn toggle_expanded(&mut self, row: usize) {
        if !self.collapsed.remove(&row) {
            self.collapsed.insert(row);
        }
        self.rebuild_visible();
    }

    pub fn reveal(&mut self, key: AssetKey) -> Option<usize> {
        let row = *self.by_key.get(&key)?;
        let mut parent = self.parents[row];
        let mut changed = false;
        while let Some(row) = parent {
            changed |= self.collapsed.remove(&row);
            parent = self.parents[row];
        }
        if changed {
            self.rebuild_visible();
        }
        self.visible.binary_search(&row).ok()
    }

    fn rebuild_visible(&mut self) {
        self.visible.clear();
        let mut row = 0;
        while row < self.rows.len() {
            self.visible.push(row);
            row = if self.collapsed.contains(&row) {
                self.ends[row]
            } else {
                row + 1
            };
        }
    }

    pub fn model_root(&self, nodes: &[SceneNode], key: AssetKey) -> Option<AssetKey> {
        let mut row = *self.by_key.get(&key)?;
        while let Some(parent) = self.parents[row] {
            row = parent;
        }
        Some(nodes[self.rows[row].0].key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(path_id: i64) -> AssetKey {
        AssetKey {
            file_index: 0,
            path_id,
        }
    }
    fn fixture() -> (Vec<SceneNode>, SceneTree) {
        let nodes = (1..=5)
            .map(|id| SceneNode {
                key: key(id),
                name: format!("node{id}"),
                parent: None,
            })
            .collect::<Vec<_>>();
        let tree = SceneTree::new(&nodes, vec![(0, 0), (1, 1), (2, 2), (3, 1), (4, 0)]);
        (nodes, tree)
    }

    #[test]
    fn checks_propagate_to_children_and_split_export_uses_topmost_checked_nodes() {
        let (nodes, mut tree) = fixture();
        tree.set_checked(0, true);
        assert_eq!(tree.checked_count, 4);
        assert_eq!(tree.checked_roots(&nodes), vec![key(1)]);
        tree.set_checked(2, false);
        assert_eq!(tree.checked_count, 1);
        assert_eq!(tree.checked_roots(&nodes), vec![key(4)]);
        tree.set_checked(1, true);
        assert_eq!(tree.checked_roots(&nodes), vec![key(2), key(4)]);
        tree.check_all(true);
        assert_eq!(tree.checked_roots(&nodes), vec![key(1), key(5)]);
        tree.check_all(false);
        assert!(tree.checked_roots(&nodes).is_empty());
    }

    #[test]
    fn reveal_expands_only_ancestors_and_collapsing_preserves_checks() {
        let (nodes, mut tree) = fixture();
        tree.set_checked(1, true);
        tree.toggle_expanded(1);
        tree.toggle_expanded(0);
        assert_eq!(tree.visible, vec![0, 4]);
        assert_eq!(tree.reveal(key(3)), Some(2));
        assert_eq!(tree.visible, vec![0, 1, 2, 3, 4]);
        assert_eq!(tree.checked_roots(&nodes), vec![key(2)]);
        assert_eq!(tree.reveal(key(999)), None);
    }
}
