//! Topological scheduling for workflow DAGs.
//!
//! Nodes are partitioned into *layers* via Kahn's algorithm: layer 0 holds
//! nodes with no dependencies, layer N holds nodes whose dependencies all sit
//! in layers `0..N`. The executor runs layers in order and the nodes *within*
//! a layer in parallel. A remaining-nodes count after the sweep detects cycles.

use std::collections::HashMap;

use crate::error::DagError;
use crate::model::Workflow;

/// Compute execution layers for a workflow.
///
/// Returns a vector of layers, each a vector of node indices into
/// [`Workflow::nodes`]. Indices (not ids) are returned so the executor can
/// borrow the nodes directly. Within a layer, node order matches declaration
/// order for deterministic display.
///
/// Errors with [`DagError::Cycle`] if the graph cannot be fully scheduled.
pub fn topological_layers(workflow: &Workflow) -> Result<Vec<Vec<usize>>, DagError> {
    let nodes = &workflow.nodes;
    let n = nodes.len();

    let index_of: HashMap<&str, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, node)| (node.id.as_str(), i))
        .collect();

    // in_degree[i] = number of unscheduled dependencies of node i.
    let mut in_degree = vec![0usize; n];
    for (i, node) in nodes.iter().enumerate() {
        // Dependencies are validated to exist during parsing; index_of is total.
        in_degree[i] = node.depends_on.len();
    }

    // Reverse adjacency: for each node, which nodes depend on it.
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, node) in nodes.iter().enumerate() {
        for dep in &node.depends_on {
            let d = index_of[dep.as_str()];
            dependents[d].push(i);
        }
    }

    let mut layers: Vec<Vec<usize>> = Vec::new();
    let mut scheduled = 0usize;

    // Seed with all zero-in-degree nodes, in declaration order.
    let mut current: Vec<usize> = (0..n).filter(|&i| in_degree[i] == 0).collect();

    while !current.is_empty() {
        scheduled += current.len();
        let mut next: Vec<usize> = Vec::new();
        for &i in &current {
            for &dependent in &dependents[i] {
                in_degree[dependent] -= 1;
                if in_degree[dependent] == 0 {
                    next.push(dependent);
                }
            }
        }
        layers.push(std::mem::take(&mut current));
        next.sort_unstable(); // keep declaration order within the layer
        current = next;
    }

    if scheduled != n {
        // The unscheduled nodes form (or feed) one or more cycles.
        let stuck: Vec<String> = (0..n)
            .filter(|&i| in_degree[i] > 0)
            .map(|i| nodes[i].id.clone())
            .collect();
        return Err(DagError::Cycle(stuck));
    }

    Ok(layers)
}
