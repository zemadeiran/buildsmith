use crate::config::BuildConfig;
use anyhow::{Result, bail};
use petgraph::graph::{DiGraph, NodeIndex};
use std::collections::HashMap;

pub struct BuildGraph {
    graph: DiGraph<String, ()>,
    name_to_index: HashMap<String, NodeIndex>,
}

impl BuildGraph {
    #[allow(dead_code)]
    pub fn from_config(config: &BuildConfig) -> Result<Self> {
        let mut graph: DiGraph<String, ()> = DiGraph::new();
        let mut name_to_index = HashMap::new();

        for name in config.tasks.keys() {
            let idx = graph.add_node(name.clone());
            name_to_index.insert(name.clone(), idx);
        }

        for (name, task) in &config.tasks {
            let target = name_to_index[name];
            for dep in &task.deps {
                let source = name_to_index[dep];
                graph.add_edge(source, target, ());
            }
        }

        // Check for cycles
        if let Err(cycle) = petgraph::algo::toposort(&graph, None) {
            let node = &graph[cycle.node_id()];
            bail!("Circular dependency detected involving task '{}'", node);
        }

        Ok(BuildGraph {
            graph,
            name_to_index,
        })
    }

    pub fn dependencies(&self, name: &str) -> Vec<String> {
        let idx = match self.name_to_index.get(name) {
            Some(i) => *i,
            None => return vec![],
        };
        self.graph
            .neighbors_directed(idx, petgraph::Direction::Incoming)
            .map(|n| self.graph[n].clone())
            .collect()
    }

    #[allow(dead_code)]
    pub fn dependents(&self, name: &str) -> Vec<String> {
        let idx = match self.name_to_index.get(name) {
            Some(i) => *i,
            None => return vec![],
        };
        self.graph
            .neighbors_directed(idx, petgraph::Direction::Outgoing)
            .map(|n| self.graph[n].clone())
            .collect()
    }

    pub fn all_tasks(&self) -> Vec<String> {
        self.graph
            .node_indices()
            .map(|i| self.graph[i].clone())
            .collect()
    }

    pub fn topological_order(&self) -> Vec<String> {
        petgraph::algo::toposort(&self.graph, None)
            .unwrap()
            .into_iter()
            .map(|i| self.graph[i].clone())
            .collect()
    }

    #[allow(dead_code)]
    pub fn task_count(&self) -> usize {
        self.graph.node_count()
    }
}
