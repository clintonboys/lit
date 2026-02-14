use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;

use anyhow::{Result, bail};

use crate::core::prompt::Prompt;

// ---------- Public types ----------

/// A node in the dependency graph
#[derive(Debug, Clone)]
pub struct DagNode {
    /// Path to this prompt file (relative to repo root)
    pub prompt_path: PathBuf,
    /// Prompts this node imports (dependencies)
    pub imports: Vec<PathBuf>,
    /// Prompts that depend on this node (reverse edges)
    pub dependents: Vec<PathBuf>,
    /// Output files this prompt produces
    #[allow(dead_code)]
    pub outputs: Vec<PathBuf>,
}

/// The dependency DAG for prompt resolution
#[derive(Debug, Clone)]
pub struct Dag {
    /// All nodes, keyed by prompt path
    nodes: HashMap<PathBuf, DagNode>,
    /// Prompts in topological order (roots first, leaves last)
    order: Vec<PathBuf>,
}

// ---------- Implementation ----------

impl Dag {
    /// Build a DAG from a set of parsed prompts.
    ///
    /// Validates:
    /// - No cycles in the dependency graph
    /// - No output conflicts (two prompts claiming the same output file)
    /// - All imports reference existing prompts
    pub fn build(prompts: &[Prompt]) -> Result<Self> {
        // Build the node map
        let mut nodes: HashMap<PathBuf, DagNode> = HashMap::new();

        for prompt in prompts {
            let node = DagNode {
                prompt_path: prompt.path.clone(),
                imports: prompt.frontmatter.imports.clone(),
                dependents: Vec::new(),
                outputs: prompt.frontmatter.outputs.clone(),
            };
            nodes.insert(prompt.path.clone(), node);
        }

        // Build reverse edges (dependents)
        let all_paths: Vec<PathBuf> = nodes.keys().cloned().collect();
        for path in &all_paths {
            let imports = nodes[path].imports.clone();
            for import in &imports {
                if let Some(dep_node) = nodes.get_mut(import) {
                    dep_node.dependents.push(path.clone());
                }
            }
        }

        // Validate imports reference existing prompts
        let mut missing_imports = Vec::new();
        for (path, node) in &nodes {
            for import in &node.imports {
                if !nodes.contains_key(import) {
                    missing_imports.push((path.clone(), import.clone()));
                }
            }
        }
        if !missing_imports.is_empty() {
            let details: Vec<String> = missing_imports
                .iter()
                .map(|(from, to)| format!("  {} imports {} (not found)", from.display(), to.display()))
                .collect();
            bail!(
                "Missing imports:\n{}",
                details.join("\n")
            );
        }

        // Validate no output conflicts
        Self::check_output_conflicts(prompts)?;

        // Topological sort (Kahn's algorithm)
        let order = Self::topological_sort(&nodes)?;

        Ok(Dag { nodes, order })
    }

    /// Get the topological generation order (roots first).
    pub fn order(&self) -> &[PathBuf] {
        &self.order
    }

    /// Get a node by its prompt path.
    pub fn get(&self, path: &PathBuf) -> Option<&DagNode> {
        self.nodes.get(path)
    }

    /// Get all nodes.
    #[allow(dead_code)]
    pub fn nodes(&self) -> &HashMap<PathBuf, DagNode> {
        &self.nodes
    }

    /// Get root nodes (prompts with no imports).
    pub fn roots(&self) -> Vec<&DagNode> {
        self.nodes
            .values()
            .filter(|n| n.imports.is_empty())
            .collect()
    }

    /// Get leaf nodes (prompts that nothing depends on).
    pub fn leaves(&self) -> Vec<&DagNode> {
        self.nodes
            .values()
            .filter(|n| n.dependents.is_empty())
            .collect()
    }

    /// Compute the regeneration set: given a set of changed prompts,
    /// return all prompts that need regeneration.
    ///
    /// This includes the changed prompts themselves plus all transitive
    /// dependents (downstream nodes whose inputs have changed).
    ///
    /// Results are returned in topological order.
    pub fn regeneration_set(&self, changed: &[PathBuf]) -> Vec<PathBuf> {
        let mut to_regen: HashSet<PathBuf> = HashSet::new();
        let mut queue: VecDeque<PathBuf> = VecDeque::new();

        // Seed the queue with changed prompts that exist in the DAG
        for path in changed {
            if self.nodes.contains_key(path) {
                to_regen.insert(path.clone());
                queue.push_back(path.clone());
            }
        }

        // BFS through dependents (downstream cascade)
        while let Some(current) = queue.pop_front() {
            if let Some(node) = self.nodes.get(&current) {
                for dependent in &node.dependents {
                    if to_regen.insert(dependent.clone()) {
                        queue.push_back(dependent.clone());
                    }
                }
            }
        }

        // Return in topological order
        self.order
            .iter()
            .filter(|p| to_regen.contains(*p))
            .cloned()
            .collect()
    }

    /// Get the total number of prompts in the DAG.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Check if the DAG is empty.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    // ---------- Internal ----------

    /// Kahn's algorithm for topological sort.
    /// Returns an error if a cycle is detected.
    fn topological_sort(nodes: &HashMap<PathBuf, DagNode>) -> Result<Vec<PathBuf>> {
        // Compute in-degree for each node: how many of its imports are in the DAG
        let mut in_degree: HashMap<PathBuf, usize> = HashMap::new();
        for (path, node) in nodes {
            let count = node.imports.iter().filter(|i| nodes.contains_key(*i)).count();
            in_degree.insert(path.clone(), count);
        }

        // Initialize queue with all nodes that have in-degree 0 (roots)
        let mut queue: VecDeque<PathBuf> = in_degree
            .iter()
            .filter(|&(_, deg)| *deg == 0)
            .map(|(path, _)| path.clone())
            .collect();

        // Sort the initial queue for deterministic ordering
        let mut sorted_queue: Vec<PathBuf> = queue.drain(..).collect();
        sorted_queue.sort();
        queue.extend(sorted_queue);

        let mut order = Vec::new();

        while let Some(current) = queue.pop_front() {
            order.push(current.clone());

            // For each dependent of current, decrease their in-degree
            if let Some(node) = nodes.get(&current) {
                // Collect and sort dependents for deterministic ordering
                let mut dependents: Vec<PathBuf> = node.dependents.clone();
                dependents.sort();

                for dependent in &dependents {
                    if let Some(deg) = in_degree.get_mut(dependent) {
                        *deg -= 1;
                        if *deg == 0 {
                            queue.push_back(dependent.clone());
                        }
                    }
                }
            }
        }

        // If we didn't process all nodes, there's a cycle
        if order.len() != nodes.len() {
            let in_cycle: Vec<String> = nodes
                .keys()
                .filter(|p| !order.contains(p))
                .map(|p| p.display().to_string())
                .collect();

            // Try to find the actual cycle path for a better error message
            let cycle_path = Self::find_cycle(nodes, &order);

            bail!(
                "Circular dependency detected. Prompts involved: [{}]{}",
                in_cycle.join(", "),
                cycle_path
                    .map(|c| format!("\n  Cycle: {}", c))
                    .unwrap_or_default()
            );
        }

        Ok(order)
    }

    /// Try to find and report a specific cycle path for better error messages.
    fn find_cycle(
        nodes: &HashMap<PathBuf, DagNode>,
        sorted: &[PathBuf],
    ) -> Option<String> {
        let sorted_set: HashSet<&PathBuf> = sorted.iter().collect();

        // Find nodes not in sorted output (they're in cycles)
        let unsorted: Vec<&PathBuf> = nodes.keys().filter(|p| !sorted_set.contains(p)).collect();

        if unsorted.is_empty() {
            return None;
        }

        // DFS from the first unsorted node to find a cycle
        let start = unsorted[0];
        let mut visited: HashSet<&PathBuf> = HashSet::new();
        let mut path: Vec<&PathBuf> = Vec::new();

        fn dfs<'a>(
            current: &'a PathBuf,
            nodes: &'a HashMap<PathBuf, DagNode>,
            visited: &mut HashSet<&'a PathBuf>,
            path: &mut Vec<&'a PathBuf>,
            sorted_set: &HashSet<&PathBuf>,
        ) -> Option<String> {
            if path.contains(&current) {
                // Found the cycle — extract it
                let cycle_start = path.iter().position(|p| *p == current).unwrap();
                let cycle: Vec<String> = path[cycle_start..]
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect();
                return Some(format!(
                    "{} → {}",
                    cycle.join(" → "),
                    current.display()
                ));
            }

            if visited.contains(current) || sorted_set.contains(current) {
                return None;
            }

            visited.insert(current);
            path.push(current);

            if let Some(node) = nodes.get(current) {
                for import in &node.imports {
                    if let Some(cycle) = dfs(import, nodes, visited, path, sorted_set) {
                        return Some(cycle);
                    }
                }
            }

            path.pop();
            None
        }

        dfs(start, nodes, &mut visited, &mut path, &sorted_set)
    }

    /// Check that no two prompts claim the same output file.
    fn check_output_conflicts(prompts: &[Prompt]) -> Result<()> {
        let mut output_owners: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();

        for prompt in prompts {
            for output in &prompt.frontmatter.outputs {
                output_owners
                    .entry(output.clone())
                    .or_default()
                    .push(prompt.path.clone());
            }
        }

        let conflicts: Vec<String> = output_owners
            .iter()
            .filter(|(_, owners)| owners.len() > 1)
            .map(|(output, owners)| {
                let owner_strs: Vec<String> =
                    owners.iter().map(|p| p.display().to_string()).collect();
                format!(
                    "  {} claimed by: [{}]",
                    output.display(),
                    owner_strs.join(", ")
                )
            })
            .collect();

        if !conflicts.is_empty() {
            bail!(
                "Output file conflicts:\n{}",
                conflicts.join("\n")
            );
        }

        Ok(())
    }
}

// ---------- Display ----------

impl std::fmt::Display for Dag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "DAG ({} nodes)", self.nodes.len())?;
        writeln!(f, "Generation order:")?;
        for (i, path) in self.order.iter().enumerate() {
            writeln!(f, "  {}. {}", i + 1, path.display())?;
        }
        Ok(())
    }
}

// ---------- Tests ----------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::LitConfig;
    use crate::core::prompt::Prompt;

    fn test_config() -> LitConfig {
        LitConfig::from_str(
            r#"
[project]
name = "test"
version = "0.1.0"
mapping = "manifest"

[language]
default = "python"
version = "3.12"

[model]
provider = "anthropic"
model = "claude-sonnet-4-5-20250929"
temperature = 0.0
"#,
        )
        .unwrap()
    }

    fn make_prompt(path: &str, outputs: &[&str], imports: &[&str]) -> Prompt {
        let outputs_yaml = if outputs.is_empty() {
            "outputs: []".to_string()
        } else {
            let items: Vec<String> = outputs.iter().map(|o| format!("  - {}", o)).collect();
            format!("outputs:\n{}", items.join("\n"))
        };

        let imports_yaml = if imports.is_empty() {
            "imports: []".to_string()
        } else {
            let items: Vec<String> = imports.iter().map(|i| format!("  - {}", i)).collect();
            format!("imports:\n{}", items.join("\n"))
        };

        let raw = format!(
            "---\n{}\n{}\n---\n\n# Test prompt\n",
            outputs_yaml, imports_yaml
        );

        let config = test_config();
        Prompt::parse(&raw, PathBuf::from(path), &config).unwrap()
    }

    #[test]
    fn test_linear_chain() {
        // A → B → C (A is root, C is leaf)
        let a = make_prompt(
            "prompts/a.prompt.md",
            &["src/a.py"],
            &[],
        );
        let b = make_prompt(
            "prompts/b.prompt.md",
            &["src/b.py"],
            &["prompts/a.prompt.md"],
        );
        let c = make_prompt(
            "prompts/c.prompt.md",
            &["src/c.py"],
            &["prompts/b.prompt.md"],
        );

        let dag = Dag::build(&[a, b, c]).unwrap();

        assert_eq!(dag.len(), 3);
        assert_eq!(dag.order(), &[
            PathBuf::from("prompts/a.prompt.md"),
            PathBuf::from("prompts/b.prompt.md"),
            PathBuf::from("prompts/c.prompt.md"),
        ]);

        // Roots and leaves
        let roots = dag.roots();
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].prompt_path, PathBuf::from("prompts/a.prompt.md"));

        let leaves = dag.leaves();
        assert_eq!(leaves.len(), 1);
        assert_eq!(leaves[0].prompt_path, PathBuf::from("prompts/c.prompt.md"));
    }

    #[test]
    fn test_diamond_dependency() {
        // A → B, A → C, B → D, C → D
        //   A
        //  / \
        // B   C
        //  \ /
        //   D
        let a = make_prompt("prompts/a.prompt.md", &["src/a.py"], &[]);
        let b = make_prompt(
            "prompts/b.prompt.md",
            &["src/b.py"],
            &["prompts/a.prompt.md"],
        );
        let c = make_prompt(
            "prompts/c.prompt.md",
            &["src/c.py"],
            &["prompts/a.prompt.md"],
        );
        let d = make_prompt(
            "prompts/d.prompt.md",
            &["src/d.py"],
            &["prompts/b.prompt.md", "prompts/c.prompt.md"],
        );

        let dag = Dag::build(&[a, b, c, d]).unwrap();

        assert_eq!(dag.len(), 4);

        // A must come first, D must come last
        let order = dag.order();
        assert_eq!(order[0], PathBuf::from("prompts/a.prompt.md"));
        assert_eq!(order[3], PathBuf::from("prompts/d.prompt.md"));

        // B and C can be in either order, but both must be between A and D
        let b_pos = order.iter().position(|p| p == &PathBuf::from("prompts/b.prompt.md")).unwrap();
        let c_pos = order.iter().position(|p| p == &PathBuf::from("prompts/c.prompt.md")).unwrap();
        assert!(b_pos > 0 && b_pos < 3);
        assert!(c_pos > 0 && c_pos < 3);
    }

    #[test]
    fn test_cycle_detection() {
        // A → B → A (cycle)
        // We need to build prompts manually since make_prompt validates imports
        // exist as .prompt.md files but doesn't check cross-references
        let a = make_prompt(
            "prompts/a.prompt.md",
            &["src/a.py"],
            &["prompts/b.prompt.md"],
        );
        let b = make_prompt(
            "prompts/b.prompt.md",
            &["src/b.py"],
            &["prompts/a.prompt.md"],
        );

        let err = Dag::build(&[a, b]).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("Circular dependency"),
            "Expected cycle error, got: {}",
            msg
        );
    }

    #[test]
    fn test_three_node_cycle() {
        // A → B → C → A
        let a = make_prompt(
            "prompts/a.prompt.md",
            &["src/a.py"],
            &["prompts/c.prompt.md"],
        );
        let b = make_prompt(
            "prompts/b.prompt.md",
            &["src/b.py"],
            &["prompts/a.prompt.md"],
        );
        let c = make_prompt(
            "prompts/c.prompt.md",
            &["src/c.py"],
            &["prompts/b.prompt.md"],
        );

        let err = Dag::build(&[a, b, c]).unwrap_err();
        assert!(
            err.to_string().contains("Circular dependency"),
            "Expected cycle error, got: {}",
            err
        );
    }

    #[test]
    fn test_independent_prompts() {
        // A and B with no dependencies between them
        let a = make_prompt("prompts/a.prompt.md", &["src/a.py"], &[]);
        let b = make_prompt("prompts/b.prompt.md", &["src/b.py"], &[]);

        let dag = Dag::build(&[a, b]).unwrap();

        assert_eq!(dag.len(), 2);
        // Both should be roots and leaves
        assert_eq!(dag.roots().len(), 2);
        assert_eq!(dag.leaves().len(), 2);
        // Both should be in the order
        assert_eq!(dag.order().len(), 2);
    }

    #[test]
    fn test_regeneration_set_linear() {
        // A → B → C, change A → regen {A, B, C}
        let a = make_prompt("prompts/a.prompt.md", &["src/a.py"], &[]);
        let b = make_prompt(
            "prompts/b.prompt.md",
            &["src/b.py"],
            &["prompts/a.prompt.md"],
        );
        let c = make_prompt(
            "prompts/c.prompt.md",
            &["src/c.py"],
            &["prompts/b.prompt.md"],
        );

        let dag = Dag::build(&[a, b, c]).unwrap();

        let regen = dag.regeneration_set(&[PathBuf::from("prompts/a.prompt.md")]);
        assert_eq!(regen.len(), 3);
        assert_eq!(regen[0], PathBuf::from("prompts/a.prompt.md"));
        assert_eq!(regen[1], PathBuf::from("prompts/b.prompt.md"));
        assert_eq!(regen[2], PathBuf::from("prompts/c.prompt.md"));
    }

    #[test]
    fn test_regeneration_set_middle_change() {
        // A → B → C, change B → regen {B, C}
        let a = make_prompt("prompts/a.prompt.md", &["src/a.py"], &[]);
        let b = make_prompt(
            "prompts/b.prompt.md",
            &["src/b.py"],
            &["prompts/a.prompt.md"],
        );
        let c = make_prompt(
            "prompts/c.prompt.md",
            &["src/c.py"],
            &["prompts/b.prompt.md"],
        );

        let dag = Dag::build(&[a, b, c]).unwrap();

        let regen = dag.regeneration_set(&[PathBuf::from("prompts/b.prompt.md")]);
        assert_eq!(regen.len(), 2);
        assert_eq!(regen[0], PathBuf::from("prompts/b.prompt.md"));
        assert_eq!(regen[1], PathBuf::from("prompts/c.prompt.md"));
    }

    #[test]
    fn test_regeneration_set_leaf_change() {
        // A → B → C, change C → regen {C} only
        let a = make_prompt("prompts/a.prompt.md", &["src/a.py"], &[]);
        let b = make_prompt(
            "prompts/b.prompt.md",
            &["src/b.py"],
            &["prompts/a.prompt.md"],
        );
        let c = make_prompt(
            "prompts/c.prompt.md",
            &["src/c.py"],
            &["prompts/b.prompt.md"],
        );

        let dag = Dag::build(&[a, b, c]).unwrap();

        let regen = dag.regeneration_set(&[PathBuf::from("prompts/c.prompt.md")]);
        assert_eq!(regen.len(), 1);
        assert_eq!(regen[0], PathBuf::from("prompts/c.prompt.md"));
    }

    #[test]
    fn test_regeneration_set_independent() {
        // A → B, C (independent). Change B → regen {B} only
        let a = make_prompt("prompts/a.prompt.md", &["src/a.py"], &[]);
        let b = make_prompt(
            "prompts/b.prompt.md",
            &["src/b.py"],
            &["prompts/a.prompt.md"],
        );
        let c = make_prompt("prompts/c.prompt.md", &["src/c.py"], &[]);

        let dag = Dag::build(&[a, b, c]).unwrap();

        let regen = dag.regeneration_set(&[PathBuf::from("prompts/b.prompt.md")]);
        assert_eq!(regen.len(), 1);
        assert_eq!(regen[0], PathBuf::from("prompts/b.prompt.md"));
    }

    #[test]
    fn test_regeneration_set_diamond() {
        // Diamond: A → B, A → C, B → D, C → D
        // Change A → regen {A, B, C, D}
        let a = make_prompt("prompts/a.prompt.md", &["src/a.py"], &[]);
        let b = make_prompt(
            "prompts/b.prompt.md",
            &["src/b.py"],
            &["prompts/a.prompt.md"],
        );
        let c = make_prompt(
            "prompts/c.prompt.md",
            &["src/c.py"],
            &["prompts/a.prompt.md"],
        );
        let d = make_prompt(
            "prompts/d.prompt.md",
            &["src/d.py"],
            &["prompts/b.prompt.md", "prompts/c.prompt.md"],
        );

        let dag = Dag::build(&[a, b, c, d]).unwrap();

        let regen = dag.regeneration_set(&[PathBuf::from("prompts/a.prompt.md")]);
        assert_eq!(regen.len(), 4);
    }

    #[test]
    fn test_regeneration_set_diamond_partial() {
        // Diamond: A → B, A → C, B → D, C → D
        // Change C → regen {C, D} (not A or B)
        let a = make_prompt("prompts/a.prompt.md", &["src/a.py"], &[]);
        let b = make_prompt(
            "prompts/b.prompt.md",
            &["src/b.py"],
            &["prompts/a.prompt.md"],
        );
        let c = make_prompt(
            "prompts/c.prompt.md",
            &["src/c.py"],
            &["prompts/a.prompt.md"],
        );
        let d = make_prompt(
            "prompts/d.prompt.md",
            &["src/d.py"],
            &["prompts/b.prompt.md", "prompts/c.prompt.md"],
        );

        let dag = Dag::build(&[a, b, c, d]).unwrap();

        let regen = dag.regeneration_set(&[PathBuf::from("prompts/c.prompt.md")]);
        assert_eq!(regen.len(), 2);
        assert!(regen.contains(&PathBuf::from("prompts/c.prompt.md")));
        assert!(regen.contains(&PathBuf::from("prompts/d.prompt.md")));
    }

    #[test]
    fn test_output_conflict_detection() {
        // Two prompts claiming the same output file
        let a = make_prompt("prompts/a.prompt.md", &["src/shared.py"], &[]);
        let b = make_prompt("prompts/b.prompt.md", &["src/shared.py"], &[]);

        let err = Dag::build(&[a, b]).unwrap_err();
        assert!(
            err.to_string().contains("Output file conflicts"),
            "Expected output conflict error, got: {}",
            err
        );
        assert!(
            err.to_string().contains("src/shared.py"),
            "Expected shared.py in error, got: {}",
            err
        );
    }

    #[test]
    fn test_missing_import_detection() {
        // A imports B which doesn't exist
        let a = make_prompt(
            "prompts/a.prompt.md",
            &["src/a.py"],
            &["prompts/nonexistent.prompt.md"],
        );

        let err = Dag::build(&[a]).unwrap_err();
        assert!(
            err.to_string().contains("Missing imports"),
            "Expected missing import error, got: {}",
            err
        );
        assert!(
            err.to_string().contains("nonexistent"),
            "Expected nonexistent in error, got: {}",
            err
        );
    }

    #[test]
    fn test_empty_dag() {
        let dag = Dag::build(&[]).unwrap();
        assert!(dag.is_empty());
        assert_eq!(dag.len(), 0);
        assert!(dag.order().is_empty());
        assert!(dag.roots().is_empty());
        assert!(dag.leaves().is_empty());
    }

    #[test]
    fn test_single_node() {
        let a = make_prompt("prompts/a.prompt.md", &["src/a.py"], &[]);
        let dag = Dag::build(&[a]).unwrap();

        assert_eq!(dag.len(), 1);
        assert_eq!(dag.order().len(), 1);
        assert_eq!(dag.roots().len(), 1);
        assert_eq!(dag.leaves().len(), 1);
    }

    #[test]
    fn test_regeneration_set_unknown_path() {
        // Passing a path not in the DAG should be ignored
        let a = make_prompt("prompts/a.prompt.md", &["src/a.py"], &[]);
        let dag = Dag::build(&[a]).unwrap();

        let regen = dag.regeneration_set(&[PathBuf::from("prompts/nonexistent.prompt.md")]);
        assert!(regen.is_empty());
    }

    #[test]
    fn test_display() {
        let a = make_prompt("prompts/a.prompt.md", &["src/a.py"], &[]);
        let b = make_prompt(
            "prompts/b.prompt.md",
            &["src/b.py"],
            &["prompts/a.prompt.md"],
        );

        let dag = Dag::build(&[a, b]).unwrap();
        let display = format!("{}", dag);
        assert!(display.contains("DAG (2 nodes)"));
        assert!(display.contains("prompts/a.prompt.md"));
        assert!(display.contains("prompts/b.prompt.md"));
    }

    #[test]
    fn test_multiple_changes() {
        // A, B (independent), C depends on A, D depends on B
        // Change both A and B → regen {A, B, C, D}
        let a = make_prompt("prompts/a.prompt.md", &["src/a.py"], &[]);
        let b = make_prompt("prompts/b.prompt.md", &["src/b.py"], &[]);
        let c = make_prompt(
            "prompts/c.prompt.md",
            &["src/c.py"],
            &["prompts/a.prompt.md"],
        );
        let d = make_prompt(
            "prompts/d.prompt.md",
            &["src/d.py"],
            &["prompts/b.prompt.md"],
        );

        let dag = Dag::build(&[a, b, c, d]).unwrap();

        let regen = dag.regeneration_set(&[
            PathBuf::from("prompts/a.prompt.md"),
            PathBuf::from("prompts/b.prompt.md"),
        ]);
        assert_eq!(regen.len(), 4);
    }
}
