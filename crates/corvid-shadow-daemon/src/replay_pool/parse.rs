use super::*;

pub fn parse_program_source(path: &Path) -> Result<(IrFile, HashMap<String, AgentInvariantInfo>)> {
    let source = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read `{}`", path.display()))?;
    let config = load_corvid_config_for(path);
    let ir = compile_to_ir_with_config_at_path(&source, path, config.as_ref()).map_err(
        |diagnostics| {
            anyhow::anyhow!(
                "{}",
                diagnostics
                    .into_iter()
                    .map(|d| d.message)
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        },
    )?;

    let tokens = lex(&source).map_err(|errs| {
        anyhow::anyhow!(
            "{}",
            errs.into_iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join("\n")
        )
    })?;
    let (file, parse_errors) = parse_file(&tokens);
    if !parse_errors.is_empty() {
        anyhow::bail!(
            "{}",
            parse_errors
                .into_iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    let dangerous_tools = file
        .decls
        .iter()
        .filter_map(|decl| match decl {
            Decl::Tool(tool) if matches!(tool.effect, Effect::Dangerous) => {
                Some(DangerousToolSpec {
                    tool: tool.name.name.clone(),
                    approval_label: approval_label_for_tool(&tool.name.name),
                })
            }
            _ => None,
        })
        .collect::<Vec<_>>();

    let mut metadata = HashMap::new();
    for agent in &ir.agents {
        let attrs = file
            .decls
            .iter()
            .find_map(|decl| match decl {
                Decl::Agent(candidate) if candidate.name.name == agent.name => {
                    Some(candidate.attributes.clone())
                }
                _ => None,
            })
            .unwrap_or_default();
        metadata.insert(
            agent.name.clone(),
            AgentInvariantInfo {
                agent: agent.name.clone(),
                replayable: AgentAttribute::is_replayable(&attrs),
                deterministic: AgentAttribute::is_deterministic(&attrs),
                grounded_return: matches!(agent.return_ty, Type::Grounded(_)),
                budget_declared: agent.cost_budget,
                dangerous_tools: dangerous_tools.clone(),
            },
        );
    }

    Ok((ir, metadata))
}

pub fn approval_label_for_tool(tool_name: &str) -> String {
    tool_name
        .split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<String>()
}
