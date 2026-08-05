use crate::config::BuildConfig;
use anyhow::{Result, bail};
use petgraph::graph::{DiGraph, NodeIndex};
use std::collections::HashMap;

#[derive(Debug)]
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{BuildConfig, TaskConfig};
    use std::collections::HashMap;

    fn make_config(tasks: &[(&str, &[&str])]) -> BuildConfig {
        let mut task_map = HashMap::new();
        for (name, deps) in tasks {
            task_map.insert(
                name.to_string(),
                TaskConfig {
                    command: format!("echo {}", name),
                    inputs: vec![],
                    outputs: vec![],
                    deps: deps.iter().map(|d| d.to_string()).collect(),
                    description: None,
                },
            );
        }
        BuildConfig {
            project: "test".to_string(),
            cache_dir: std::path::PathBuf::from(".buildsmith/cache"),
            tasks: task_map,
        }
    }

    #[test]
    fn test_graph_no_deps() {
        let config = make_config(&[("a", &[]), ("b", &[]), ("c", &[])]);
        let graph = BuildGraph::from_config(&config).unwrap();
        assert_eq!(graph.all_tasks().len(), 3);
        assert!(graph.dependencies("a").is_empty());
    }

    #[test]
    fn test_graph_with_deps() {
        let config = make_config(&[("build", &[]), ("test", &["build"])]);
        let graph = BuildGraph::from_config(&config).unwrap();
        let deps = graph.dependencies("test");
        assert_eq!(deps, vec!["build"]);
    }

    #[test]
    fn test_graph_cycle_detection() {
        let config = make_config(&[("a", &["b"]), ("b", &["a"])]);
        let result = BuildGraph::from_config(&config);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("Circular dependency"));
    }

    #[test]
    fn test_graph_topological_order() {
        let config = make_config(&[("build", &[]), ("test", &["build"]), ("deploy", &["test"])]);
        let graph = BuildGraph::from_config(&config).unwrap();
        let order = graph.topological_order();
        let build_idx = order.iter().position(|t| t == "build").unwrap();
        let test_idx = order.iter().position(|t| t == "test").unwrap();
        let deploy_idx = order.iter().position(|t| t == "deploy").unwrap();
        assert!(build_idx < test_idx);
        assert!(test_idx < deploy_idx);
    }

    #[test]
    fn test_graph_unknown_task_deps() {
        let deps = graph_dependencies_unknown();
        assert!(deps.is_empty());
    }

    fn graph_dependencies_unknown() -> Vec<String> {
        let config = make_config(&[("a", &[])]);
        let graph = BuildGraph::from_config(&config).unwrap();
        graph.dependencies("nonexistent")
    }
}
