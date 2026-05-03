use super::*;

pub fn numeric_constraint_value(constraint: &EffectConstraint) -> Option<f64> {
    match constraint.value.as_ref()? {
        DimensionValue::Cost(value) => Some(*value),
        DimensionValue::Number(value) => Some(*value),
        _ => None,
    }
}

pub fn cost_path_for_dimension(tree: &CostTreeNode, dimension: &str) -> Vec<String> {
    match tree.kind {
        CostNodeKind::Agent | CostNodeKind::Sequence | CostNodeKind::Condition => tree
            .children
            .iter()
            .filter(|child| child.costs.get(dimension).copied().unwrap_or(0.0) > 0.0)
            .flat_map(|child| cost_path_for_dimension(child, dimension))
            .collect(),
        CostNodeKind::Branch => tree
            .children
            .iter()
            .max_by(|left, right| {
                left.costs
                    .get(dimension)
                    .copied()
                    .unwrap_or(0.0)
                    .partial_cmp(&right.costs.get(dimension).copied().unwrap_or(0.0))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|child| cost_path_for_dimension(child, dimension))
            .unwrap_or_default(),
        CostNodeKind::Loop { iterations } => {
            let mut path = tree
                .children
                .get(0)
                .map(|child| cost_path_for_dimension(child, dimension))
                .unwrap_or_default();
            if let Some(iterations) = iterations {
                if let Some(last) = path.last_mut() {
                    *last = format!("{last} × {iterations} iterations");
                }
            }
            path
        }
        CostNodeKind::Tool | CostNodeKind::Prompt => vec![format!(
            "{} ({})",
            tree.name,
            format_numeric_dimension(dimension, tree.costs.get(dimension).copied().unwrap_or(0.0))
        )],
    }
}
