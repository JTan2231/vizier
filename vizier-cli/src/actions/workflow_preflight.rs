use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::Path;

use crate::workflow_templates::{self, ResolvedWorkflowSource, WorkflowTemplateInputSpec};

pub(crate) struct PreparedWorkflowTemplate {
    pub(crate) source: ResolvedWorkflowSource,
    pub(crate) template: vizier_core::workflow_template::WorkflowTemplate,
}

pub(crate) fn prepare_workflow_template(
    project_root: &Path,
    flow: &str,
    inputs: &[String],
    set: &[String],
    cfg: &vizier_core::config::Config,
) -> Result<PreparedWorkflowTemplate, Box<dyn std::error::Error>> {
    let source = workflow_templates::resolve_workflow_source(project_root, flow, cfg)?;
    let input_spec = workflow_templates::load_template_input_spec(&source)?;
    let mut set_overrides = parse_set_overrides(set)?;
    apply_named_input_aliases(&source, &input_spec, &mut set_overrides)?;
    apply_positional_inputs(&source, &input_spec, inputs, &mut set_overrides)?;
    let mut template = workflow_templates::load_template_with_params(&source, &set_overrides)?;
    validate_entrypoint_input_requirements(&source, &input_spec, &template)?;
    inline_plan_persist_spec_files(project_root, &mut template)?;
    Ok(PreparedWorkflowTemplate { source, template })
}

fn parse_set_overrides(
    values: &[String],
) -> Result<BTreeMap<String, String>, Box<dyn std::error::Error>> {
    let mut out = BTreeMap::new();
    for raw in values {
        let trimmed = raw.trim();
        let Some((key, value)) = trimmed.split_once('=') else {
            return Err(format!("invalid --set value `{raw}`; expected KEY=VALUE").into());
        };
        let key = key.trim();
        if key.is_empty() {
            return Err(format!("invalid --set value `{raw}`; key cannot be empty").into());
        }
        out.insert(key.to_string(), value.to_string());
    }
    Ok(out)
}

fn apply_named_input_aliases(
    source: &ResolvedWorkflowSource,
    input_spec: &WorkflowTemplateInputSpec,
    set_overrides: &mut BTreeMap<String, String>,
) -> Result<(), Box<dyn std::error::Error>> {
    if input_spec.named.is_empty() {
        return Ok(());
    }

    let declared_params = input_spec
        .params
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let aliases = input_spec
        .named
        .iter()
        .map(|(alias, target)| (alias.trim().replace('-', "_"), target.trim().to_string()))
        .collect::<Vec<_>>();

    for (alias, target) in &aliases {
        if alias.is_empty() {
            return Err(format!(
                "workflow `{}` has an empty [cli].named alias key",
                source.selector
            )
            .into());
        }
        if target.is_empty() {
            return Err(format!(
                "workflow `{}` alias `{alias}` has an empty [cli].named target",
                source.selector
            )
            .into());
        }
        if !declared_params.contains(target.as_str()) {
            return Err(format!(
                "workflow `{}` alias `{alias}` maps to unknown parameter `{target}`",
                source.selector
            )
            .into());
        }
    }

    for (alias, target) in aliases {
        if alias == target {
            continue;
        }

        let Some(value) = set_overrides.remove(&alias) else {
            continue;
        };

        if set_overrides.contains_key(&target) {
            return Err(format!(
                "workflow parameter `{target}` was provided multiple ways (`--{}` alias and explicit override)",
                alias.replace('_', "-")
            )
            .into());
        }
        set_overrides.insert(target, value);
    }

    Ok(())
}

fn apply_positional_inputs(
    source: &ResolvedWorkflowSource,
    input_spec: &WorkflowTemplateInputSpec,
    positional_values: &[String],
    set_overrides: &mut BTreeMap<String, String>,
) -> Result<(), Box<dyn std::error::Error>> {
    if positional_values.is_empty() {
        return Ok(());
    }

    if input_spec.positional.is_empty() {
        return Err(format!(
            "workflow `{}` does not define positional inputs; use named flags (for example `--param value`) or `--set key=value`",
            source.selector
        )
        .into());
    }

    if positional_values.len() > input_spec.positional.len() {
        return Err(format!(
            "workflow `{}` accepts at most {} positional input(s): {}",
            source.selector,
            input_spec.positional.len(),
            input_spec.positional.join(", ")
        )
        .into());
    }

    let declared_params = input_spec
        .params
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    for key in &input_spec.positional {
        if key.trim().is_empty() {
            return Err(format!(
                "workflow `{}` has an empty positional mapping entry",
                source.selector
            )
            .into());
        }
        if !declared_params.contains(key.as_str()) {
            return Err(format!(
                "workflow `{}` positional input `{key}` is not declared in [params]",
                source.selector
            )
            .into());
        }
    }

    for (index, value) in positional_values.iter().enumerate() {
        let key = &input_spec.positional[index];
        if set_overrides.contains_key(key) {
            return Err(format!(
                "workflow parameter `{key}` was provided multiple ways (positional input {} and named override)",
                index + 1
            )
            .into());
        }
        set_overrides.insert(key.clone(), value.clone());
    }

    Ok(())
}

fn inline_plan_persist_spec_files(
    project_root: &Path,
    template: &mut vizier_core::workflow_template::WorkflowTemplate,
) -> Result<(), Box<dyn std::error::Error>> {
    for node in &mut template.nodes {
        if node.uses != "cap.env.builtin.plan.persist" {
            continue;
        }

        let spec_source = node
            .args
            .get("spec_source")
            .map(|value| value.trim().to_ascii_lowercase())
            .unwrap_or_else(|| "inline".to_string());
        if !matches!(spec_source.as_str(), "inline" | "stdin") {
            continue;
        }

        let has_spec_text = node
            .args
            .get("spec_text")
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false);
        if has_spec_text {
            continue;
        }

        let Some(spec_file) = node
            .args
            .get("spec_file")
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
        else {
            continue;
        };

        let spec_path = project_root.join(spec_file);
        let spec_text = fs::read_to_string(&spec_path).map_err(|err| {
            format!(
                "workflow node `{}` could not read spec file `{}` during queue-time validation: {err}",
                node.id,
                spec_path.display()
            )
        })?;
        node.args.insert("spec_text".to_string(), spec_text);
    }

    Ok(())
}

fn validate_entrypoint_input_requirements(
    source: &ResolvedWorkflowSource,
    input_spec: &WorkflowTemplateInputSpec,
    template: &vizier_core::workflow_template::WorkflowTemplate,
) -> Result<(), Box<dyn std::error::Error>> {
    if template.nodes.is_empty() {
        return Ok(());
    }

    let mut incoming_success = HashSet::<String>::new();
    for node in &template.nodes {
        for target in &node.on.succeeded {
            incoming_success.insert(target.clone());
        }
    }

    let mut resolved_after = BTreeMap::new();
    for node in &template.nodes {
        resolved_after.insert(node.id.clone(), format!("preflight-{}", node.id));
    }

    let declared_params = input_spec
        .params
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();

    for node in &template.nodes {
        if !node.after.is_empty() || incoming_success.contains(&node.id) {
            continue;
        }

        let compiled = vizier_core::workflow_template::compile_workflow_node(
            template,
            &node.id,
            &resolved_after,
        )
        .map_err(|err| {
            format!(
                "workflow `{}` failed entry-node preflight for `{}`: {err}",
                source.selector, node.id
            )
        })?;

        let Some(operation) = compiled.executor_operation.as_deref() else {
            continue;
        };
        let Some(required_arg_keys) =
            vizier_core::workflow_template::executor_non_empty_any_of_arg_keys(operation)
        else {
            continue;
        };

        let has_required_value = required_arg_keys.iter().any(|key| {
            node.args
                .get(*key)
                .map(|value| !value.trim().is_empty())
                .unwrap_or(false)
        });
        if has_required_value {
            continue;
        }

        let node_arg_keys = required_arg_keys
            .iter()
            .filter(|key| node.args.contains_key(**key))
            .map(|key| (*key).to_string())
            .collect::<Vec<_>>();

        let mut required_inputs = node_arg_keys
            .iter()
            .filter(|key| declared_params.contains(key.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        if required_inputs.is_empty() {
            required_inputs = if node_arg_keys.is_empty() {
                required_arg_keys
                    .iter()
                    .map(|key| (*key).to_string())
                    .collect::<Vec<_>>()
            } else {
                node_arg_keys
            };
        }
        required_inputs.sort_by_key(|key| {
            input_spec
                .positional
                .iter()
                .position(|entry| entry == key)
                .unwrap_or(usize::MAX)
        });

        return Err(build_entrypoint_input_error(source, input_spec, &required_inputs).into());
    }

    Ok(())
}

fn build_entrypoint_input_error(
    source: &ResolvedWorkflowSource,
    input_spec: &WorkflowTemplateInputSpec,
    required_inputs: &[String],
) -> String {
    let flow_label = source
        .command_alias
        .as_ref()
        .map(|alias| alias.as_str().to_string())
        .unwrap_or_else(|| source.selector.clone());

    let mut usage_params = ordered_cli_params(input_spec);
    if usage_params.is_empty() {
        usage_params = required_inputs.to_vec();
        usage_params.sort_by_key(|param| {
            input_spec
                .positional
                .iter()
                .position(|entry| entry == param)
                .unwrap_or(usize::MAX)
        });
        usage_params.dedup();
    }

    let usage = if usage_params.is_empty() {
        format!("vizier run {flow_label} [--set <KEY=VALUE>]...")
    } else {
        let flags = usage_params
            .iter()
            .map(|param| {
                let label = cli_label_for_param(input_spec, param);
                format!(
                    "[--{} <{}>]",
                    kebab_case_key(&label),
                    kebab_case_key(&label)
                )
            })
            .collect::<Vec<_>>();
        format!("vizier run {flow_label} {}", flags.join(" "))
    };

    let named_example = named_example(&flow_label, input_spec, &usage_params);
    let positional_example = positional_example(&flow_label, input_spec);

    let mut lines = vec![
        format!("error: missing required input for workflow `{flow_label}`"),
        format!("usage: {usage}"),
        format!("example: {named_example}"),
    ];
    if let Some(example) = positional_example
        && example != named_example
    {
        lines.push(format!("example (positional): {example}"));
    }
    lines.push(format!("hint: vizier run {flow_label} --help"));

    lines.join("\n")
}

fn ordered_cli_params(input_spec: &WorkflowTemplateInputSpec) -> Vec<String> {
    let mut ordered = Vec::<String>::new();
    for param in &input_spec.positional {
        if !ordered.contains(param) {
            ordered.push(param.clone());
        }
    }
    for target in input_spec.named.values() {
        if !ordered.contains(target) {
            ordered.push(target.clone());
        }
    }
    for param in &input_spec.params {
        if !ordered.contains(param) {
            ordered.push(param.clone());
        }
    }
    ordered
}

fn named_example(
    flow_label: &str,
    input_spec: &WorkflowTemplateInputSpec,
    ordered_params: &[String],
) -> String {
    if ordered_params.is_empty() {
        return format!("vizier run {flow_label} --set key=value");
    }

    let mut parts = vec![format!("vizier run {flow_label}")];
    let take = ordered_params.len().clamp(1, 2);
    for param in ordered_params.iter().take(take) {
        let label = cli_label_for_param(input_spec, param);
        parts.push(format!(
            "--{} {}",
            kebab_case_key(&label),
            example_value(input_spec, param)
        ));
    }

    parts.join(" ")
}

fn positional_example(flow_label: &str, input_spec: &WorkflowTemplateInputSpec) -> Option<String> {
    if input_spec.positional.is_empty() {
        return None;
    }

    let values = input_spec
        .positional
        .iter()
        .take(2)
        .map(|param| example_value(input_spec, param))
        .collect::<Vec<_>>();
    if values.is_empty() {
        None
    } else {
        Some(format!("vizier run {flow_label} {}", values.join(" ")))
    }
}

fn preferred_cli_alias_for_param<'a>(
    input_spec: &'a WorkflowTemplateInputSpec,
    param: &str,
) -> Option<&'a str> {
    input_spec.named.iter().find_map(|(alias, target)| {
        if target == param {
            Some(alias.as_str())
        } else {
            None
        }
    })
}

fn cli_label_for_param(input_spec: &WorkflowTemplateInputSpec, param: &str) -> String {
    preferred_cli_alias_for_param(input_spec, param)
        .unwrap_or(param)
        .to_string()
}

fn kebab_case_key(value: &str) -> String {
    value.trim().replace('_', "-")
}

fn example_value(input_spec: &WorkflowTemplateInputSpec, param: &str) -> String {
    let label = cli_label_for_param(input_spec, param).to_ascii_lowercase();
    if label.contains("file") || label.contains("path") {
        "LIBRARY.md".to_string()
    } else if label.contains("name") || label.contains("slug") {
        "my-change".to_string()
    } else if label.contains("target") {
        "main".to_string()
    } else if label.contains("branch") {
        "draft/my-change".to_string()
    } else {
        format!("example-{}", kebab_case_key(&label))
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;
    use vizier_core::workflow_template::{WorkflowNode, WorkflowTemplate};

    use super::*;

    fn test_source() -> ResolvedWorkflowSource {
        ResolvedWorkflowSource {
            selector: "draft".to_string(),
            path: std::path::PathBuf::from(".vizier/workflows/draft.hcl"),
            command_alias: None,
        }
    }

    #[test]
    fn parse_set_overrides_accepts_last_write_wins() {
        let parsed = parse_set_overrides(&[
            "one=1".to_string(),
            "two=2".to_string(),
            "one=3".to_string(),
        ])
        .expect("parse overrides");

        assert_eq!(parsed.get("one"), Some(&"3".to_string()));
        assert_eq!(parsed.get("two"), Some(&"2".to_string()));
    }

    #[test]
    fn parse_set_overrides_rejects_missing_equals() {
        let err = parse_set_overrides(&["missing".to_string()]).expect_err("expected error");
        assert!(
            err.to_string().contains("expected KEY=VALUE"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn apply_named_input_aliases_maps_alias_to_declared_param() {
        let source = test_source();
        let input_spec = WorkflowTemplateInputSpec {
            params: vec!["slug".to_string(), "spec_file".to_string()],
            positional: vec![],
            named: BTreeMap::from([
                ("name".to_string(), "slug".to_string()),
                ("file".to_string(), "spec_file".to_string()),
            ]),
        };
        let mut overrides = BTreeMap::from([
            ("name".to_string(), "my-change".to_string()),
            ("file".to_string(), "specs/DEFAULT.md".to_string()),
        ]);

        apply_named_input_aliases(&source, &input_spec, &mut overrides).expect("map named aliases");

        assert_eq!(overrides.get("slug"), Some(&"my-change".to_string()));
        assert_eq!(
            overrides.get("spec_file"),
            Some(&"specs/DEFAULT.md".to_string())
        );
        assert!(
            !overrides.contains_key("name") && !overrides.contains_key("file"),
            "aliases should be replaced by canonical params: {overrides:?}"
        );
    }

    #[test]
    fn apply_named_input_aliases_rejects_alias_plus_explicit_target() {
        let source = test_source();
        let input_spec = WorkflowTemplateInputSpec {
            params: vec!["slug".to_string()],
            positional: vec![],
            named: BTreeMap::from([("name".to_string(), "slug".to_string())]),
        };
        let mut overrides = BTreeMap::from([
            ("name".to_string(), "alpha".to_string()),
            ("slug".to_string(), "beta".to_string()),
        ]);

        let err =
            apply_named_input_aliases(&source, &input_spec, &mut overrides).expect_err("conflict");
        assert!(
            err.to_string().contains("provided multiple ways"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn inline_plan_persist_spec_files_materializes_spec_text() {
        let temp = TempDir::new().expect("temp dir");
        let spec_rel = "specs/LOCAL.md";
        let spec_path = temp.path().join(spec_rel);
        std::fs::create_dir_all(spec_path.parent().expect("parent dir")).expect("mkdir");
        std::fs::write(&spec_path, "Local draft spec\nline two\n").expect("write spec");

        let mut template = WorkflowTemplate {
            id: "template.test".to_string(),
            version: "v1".to_string(),
            params: BTreeMap::new(),
            node_lock_scope_contexts: BTreeMap::new(),
            policy: Default::default(),
            artifact_contracts: Vec::new(),
            nodes: vec![WorkflowNode {
                id: "persist_plan".to_string(),
                kind: Default::default(),
                uses: "cap.env.builtin.plan.persist".to_string(),
                args: BTreeMap::from([
                    ("spec_source".to_string(), "inline".to_string()),
                    ("spec_text".to_string(), "".to_string()),
                    ("spec_file".to_string(), spec_rel.to_string()),
                ]),
                after: Vec::new(),
                needs: Vec::new(),
                produces: Default::default(),
                locks: Vec::new(),
                preconditions: Vec::new(),
                gates: Vec::new(),
                retry: Default::default(),
                on: Default::default(),
            }],
        };

        inline_plan_persist_spec_files(temp.path(), &mut template).expect("inline spec file");
        assert_eq!(
            template.nodes[0].args.get("spec_text"),
            Some(&"Local draft spec\nline two\n".to_string())
        );
    }

    #[test]
    fn inline_plan_persist_spec_files_respects_explicit_file_source() {
        let temp = TempDir::new().expect("temp dir");

        let mut template = WorkflowTemplate {
            id: "template.test".to_string(),
            version: "v1".to_string(),
            params: BTreeMap::new(),
            node_lock_scope_contexts: BTreeMap::new(),
            policy: Default::default(),
            artifact_contracts: Vec::new(),
            nodes: vec![WorkflowNode {
                id: "persist_plan".to_string(),
                kind: Default::default(),
                uses: "cap.env.builtin.plan.persist".to_string(),
                args: BTreeMap::from([
                    ("spec_source".to_string(), "file".to_string()),
                    ("spec_text".to_string(), "".to_string()),
                    ("spec_file".to_string(), "specs/LOCAL.md".to_string()),
                ]),
                after: Vec::new(),
                needs: Vec::new(),
                produces: Default::default(),
                locks: Vec::new(),
                preconditions: Vec::new(),
                gates: Vec::new(),
                retry: Default::default(),
                on: Default::default(),
            }],
        };

        inline_plan_persist_spec_files(temp.path(), &mut template).expect("skip file source");
        assert_eq!(
            template.nodes[0].args.get("spec_text"),
            Some(&"".to_string())
        );
    }
}
