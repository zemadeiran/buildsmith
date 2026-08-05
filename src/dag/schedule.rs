use crate::dag::BuildGraph;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ScheduledTask {
    pub name: String,
    pub level: usize,
}

pub struct Schedule {
    pub levels: Vec<Vec<String>>,
}

impl Schedule {
    pub fn from_graph(graph: &BuildGraph) -> Self {
        let topo = graph.topological_order();
        let mut level_map: HashMap<String, usize> = HashMap::new();

        for name in &topo {
            let deps = graph.dependencies(name);
            let max_dep_level = deps
                .iter()
                .map(|d| level_map.get(d).copied().unwrap_or(0))
                .max()
                .unwrap_or(0);
            level_map.insert(name.clone(), max_dep_level + 1);
        }

        let max_level = level_map.values().copied().max().unwrap_or(0);
        let mut levels: Vec<Vec<String>> = vec![vec![]; max_level];

        for (name, &level) in &level_map {
            levels[level - 1].push(name.clone());
        }

        // Sort within each level for deterministic ordering
        for level in &mut levels {
            level.sort();
        }

        Schedule { levels }
    }

    pub fn for_task(graph: &BuildGraph, task_name: &str) -> Self {
        let mut visited = HashSet::new();
        collect_deps(graph, task_name, &mut visited);

        let topo = graph.topological_order();
        let mut level_map: HashMap<String, usize> = HashMap::new();

        for name in &topo {
            if !visited.contains(name) {
                continue;
            }
            let deps = graph.dependencies(name);
            let max_dep_level = deps
                .iter()
                .filter(|d| visited.contains(*d))
                .map(|d| level_map.get(d).copied().unwrap_or(0))
                .max()
                .unwrap_or(0);
            level_map.insert(name.clone(), max_dep_level + 1);
        }

        let max_level = level_map.values().copied().max().unwrap_or(0);
        let mut levels: Vec<Vec<String>> = vec![vec![]; max_level];

        for (name, &level) in &level_map {
            levels[level - 1].push(name.clone());
        }

        for level in &mut levels {
            level.sort();
        }

        Schedule { levels }
    }

    #[allow(dead_code)]
    pub fn total_tasks(&self) -> usize {
        self.levels.iter().map(|l| l.len()).sum()
    }
}

fn collect_deps(graph: &BuildGraph, name: &str, visited: &mut HashSet<String>) {
    if visited.contains(name) {
        return;
    }
    visited.insert(name.to_string());
    for dep in graph.dependencies(name) {
        collect_deps(graph, &dep, visited);
    }
}
