use std::collections::HashSet;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use crate::agent::{AgentRunner, ScriptRunner};

use super::{
    AgentOutputHandling, AgentOverrides, AgentRuntimeOptions, AgentRuntimeResolution, BackendKind,
    CommandScope, Config, DocumentationSettings, ProfileScope, PromptKind, PromptOverrides,
    PromptSelection, ResolvedAgentRuntime, backend_kind_for_selector, default_selector_for_backend,
};

#[derive(Clone)]
pub struct AgentSettings {
    pub profile_scope: ProfileScope,
    pub scope: Option<CommandScope>,
    pub selector: String,
    pub backend: BackendKind,
    pub runner: Option<Arc<dyn AgentRunner>>,
    pub agent_runtime: ResolvedAgentRuntime,
    pub documentation: DocumentationSettings,
    pub prompt: Option<PromptSelection>,
    pub cli_override: Option<AgentOverrides>,
}

impl AgentSettings {
    pub fn for_prompt(
        &self,
        kind: PromptKind,
    ) -> Result<AgentSettings, Box<dyn std::error::Error>> {
        match self.scope {
            Some(scope) => super::resolve_prompt_profile(
                &super::get_config(),
                scope,
                kind,
                self.cli_override.as_ref(),
            ),
            None => super::resolve_default_prompt_profile(
                &super::get_config(),
                kind,
                self.cli_override.as_ref(),
            ),
        }
    }

    pub fn prompt_selection(&self) -> Option<&PromptSelection> {
        self.prompt.as_ref()
    }

    pub fn agent_runner(&self) -> Result<&Arc<dyn AgentRunner>, Box<dyn std::error::Error>> {
        self.runner.as_ref().ok_or_else(|| {
            format!(
                "agent scope `{}` requires an agent backend runner, but none was resolved",
                self.profile_scope.as_str()
            )
            .into()
        })
    }
}

pub fn resolve_agent_settings(
    cfg: &Config,
    scope: CommandScope,
    cli_override: Option<&AgentOverrides>,
) -> Result<AgentSettings, Box<dyn std::error::Error>> {
    let mut builder = AgentSettingsBuilder::new(cfg);

    if !cfg.agent_defaults.is_empty() {
        builder.apply(&cfg.agent_defaults);
    }

    if let Some(overrides) = cfg.agent_scopes.get(&scope) {
        builder.apply(overrides);
    }

    if let Some(overrides) = cli_override
        && !overrides.is_empty()
    {
        builder.apply_cli_override(overrides);
    }

    builder.build(
        ProfileScope::Command(scope),
        Some(scope),
        None,
        cli_override,
    )
}

pub fn resolve_default_agent_settings(
    cfg: &Config,
    cli_override: Option<&AgentOverrides>,
) -> Result<AgentSettings, Box<dyn std::error::Error>> {
    let mut builder = AgentSettingsBuilder::new(cfg);

    if !cfg.agent_defaults.is_empty() {
        builder.apply(&cfg.agent_defaults);
    }

    if let Some(overrides) = cli_override
        && !overrides.is_empty()
    {
        builder.apply_cli_override(overrides);
    }

    builder.build(ProfileScope::Default, None, None, cli_override)
}

pub fn resolve_prompt_profile(
    cfg: &Config,
    scope: CommandScope,
    kind: PromptKind,
    cli_override: Option<&AgentOverrides>,
) -> Result<AgentSettings, Box<dyn std::error::Error>> {
    let mut builder = AgentSettingsBuilder::new(cfg);

    if !cfg.agent_defaults.is_empty() {
        builder.apply(&cfg.agent_defaults);
    }

    if let Some(scope_overrides) = cfg.agent_scopes.get(&scope) {
        builder.apply(scope_overrides);
    }

    if let Some(default_prompt) = cfg.agent_defaults.prompt_overrides.get(&kind) {
        builder.apply_prompt_overrides(default_prompt);
    }

    if let Some(scoped_prompt) = cfg
        .agent_scopes
        .get(&scope)
        .and_then(|scope_overrides| scope_overrides.prompt_overrides.get(&kind))
    {
        builder.apply_prompt_overrides(scoped_prompt);
    }

    if let Some(overrides) = cli_override
        && !overrides.is_empty()
    {
        builder.apply_cli_override(overrides);
    }

    let prompt =
        if kind == PromptKind::Documentation && !builder.documentation.use_documentation_prompt {
            None
        } else {
            Some(cfg.prompt_for_command(scope, kind))
        };
    builder.build(
        ProfileScope::Command(scope),
        Some(scope),
        prompt,
        cli_override,
    )
}

pub fn resolve_default_prompt_profile(
    cfg: &Config,
    kind: PromptKind,
    cli_override: Option<&AgentOverrides>,
) -> Result<AgentSettings, Box<dyn std::error::Error>> {
    let mut builder = AgentSettingsBuilder::new(cfg);

    if !cfg.agent_defaults.is_empty() {
        builder.apply(&cfg.agent_defaults);
    }

    if let Some(default_prompt) = cfg.agent_defaults.prompt_overrides.get(&kind) {
        builder.apply_prompt_overrides(default_prompt);
    }

    if let Some(overrides) = cli_override
        && !overrides.is_empty()
    {
        builder.apply_cli_override(overrides);
    }

    let prompt =
        if kind == PromptKind::Documentation && !builder.documentation.use_documentation_prompt {
            None
        } else {
            Some(cfg.prompt_for_default(kind))
        };
    builder.build(ProfileScope::Default, None, prompt, cli_override)
}

#[derive(Clone)]
struct AgentSettingsBuilder {
    selector: String,
    backend: BackendKind,
    agent_runtime: AgentRuntimeOptions,
    documentation: DocumentationSettings,
}

impl AgentSettingsBuilder {
    fn new(cfg: &Config) -> Self {
        let selector = cfg.agent_selector.clone();
        Self {
            selector: selector.clone(),
            backend: backend_kind_for_selector(&selector),
            agent_runtime: cfg.agent_runtime.clone(),
            documentation: DocumentationSettings::default(),
        }
    }

    fn apply(&mut self, overrides: &AgentOverrides) {
        if let Some(selector) = overrides.selector.as_ref() {
            self.set_selector(selector);
        }

        if let Some(runtime) = overrides.agent_runtime.as_ref() {
            if let Some(label) = runtime.label.as_ref() {
                self.agent_runtime.label = Some(label.clone());
            }

            if let Some(command) = runtime.command.as_ref() {
                self.agent_runtime.command = command.clone();
            }

            if let Some(filter) = runtime.progress_filter.as_ref() {
                self.agent_runtime.progress_filter = Some(filter.clone());
            }

            if let Some(output) = runtime.output.as_ref() {
                self.agent_runtime.output = *output;
            }
        }

        overrides.documentation.apply_to(&mut self.documentation);
    }

    fn apply_cli_override(&mut self, overrides: &AgentOverrides) {
        if let Some(selector) = overrides.selector.as_ref() {
            self.set_selector(selector);
        }

        if let Some(runtime) = overrides.agent_runtime.as_ref() {
            if let Some(label) = runtime.label.as_ref() {
                self.agent_runtime.label = Some(label.clone());
            }

            if let Some(command) = runtime.command.as_ref() {
                self.agent_runtime.command = command.clone();
            }

            if let Some(filter) = runtime.progress_filter.as_ref() {
                self.agent_runtime.progress_filter = Some(filter.clone());
            }

            if let Some(output) = runtime.output.as_ref() {
                self.agent_runtime.output = *output;
            }

            if let Some(enable_script_wrapper) = runtime.enable_script_wrapper {
                self.agent_runtime.enable_script_wrapper = enable_script_wrapper;
            }
        }

        overrides.documentation.apply_to(&mut self.documentation);
    }

    fn apply_prompt_overrides(&mut self, overrides: &PromptOverrides) {
        if let Some(agent) = overrides.agent_overrides() {
            self.apply(agent);
        }
    }

    fn set_selector<S: AsRef<str>>(&mut self, selector: S) {
        let normalized = selector.as_ref().trim().to_ascii_lowercase();
        if normalized.is_empty() {
            return;
        }
        self.backend = backend_kind_for_selector(&normalized);
        self.selector = normalized;
    }

    fn build(
        &self,
        profile_scope: ProfileScope,
        scope: Option<CommandScope>,
        prompt: Option<PromptSelection>,
        cli_override: Option<&AgentOverrides>,
    ) -> Result<AgentSettings, Box<dyn std::error::Error>> {
        let agent_runtime = self.agent_runtime.normalized_for_selector(&self.selector);

        let resolved_runtime =
            resolve_agent_runtime(agent_runtime.clone(), &self.selector, self.backend)?;

        Ok(AgentSettings {
            profile_scope,
            scope,
            selector: self.selector.clone(),
            backend: self.backend,
            runner: resolve_agent_runner(self.backend)?,
            agent_runtime: resolved_runtime,
            documentation: self.documentation.clone(),
            prompt,
            cli_override: cli_override.cloned(),
        })
    }
}

fn command_label(command: &[String]) -> Option<String> {
    let candidate = PathBuf::from(command.first()?);
    let stem = candidate.file_stem()?.to_string_lossy().to_string();
    if stem.is_empty() { None } else { Some(stem) }
}

// Attach a bundled progress filter for any agent label that ships one (codex, gemini,
// or custom shims), so wrapped output stays consistent without per-backend branching.
fn default_progress_filter_for_label(label: &str) -> Option<Vec<String>> {
    bundled_progress_filter(label).map(|path| vec![path.display().to_string()])
}

fn bundled_agent_shim_dir_candidates() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    if let Ok(dir) = std::env::var("VIZIER_AGENT_SHIMS_DIR") {
        let trimmed = dir.trim();
        if !trimmed.is_empty() {
            dirs.push(PathBuf::from(trimmed));
        }
    }

    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        dirs.push(dir.join("agents"));
        if let Some(prefix) = dir.parent() {
            dirs.push(prefix.join("share").join("vizier").join("agents"));
        }
    }

    let workspace_agents = PathBuf::from("examples").join("agents");
    dirs.push(workspace_agents);

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if let Some(workspace_root) = manifest_dir.parent() {
        dirs.push(workspace_root.join("examples").join("agents"));
    }

    dirs.retain(|path| path.is_dir());
    dirs
}

fn bundled_agent_command(label: &str) -> Option<PathBuf> {
    find_first_in_shim_dirs(vec![
        format!("{label}/agent.sh"),
        format!("{label}.sh"), // backward compatibility
    ])
}

fn bundled_progress_filter(label: &str) -> Option<PathBuf> {
    find_first_in_shim_dirs(vec![
        format!("{label}/filter.sh"),
        format!("{label}-filter.sh"), // backward compatibility
    ])
}

fn find_in_shim_dirs(filename: &str) -> Option<PathBuf> {
    let mut seen: HashSet<PathBuf> = HashSet::new();
    for dir in bundled_agent_shim_dir_candidates() {
        if !seen.insert(dir.clone()) {
            continue;
        }
        let candidate = dir.join(filename);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn find_first_in_shim_dirs(candidates: Vec<String>) -> Option<PathBuf> {
    for name in candidates {
        if let Some(path) = find_in_shim_dirs(&name) {
            return Some(path);
        }
    }
    None
}

pub(crate) fn resolve_agent_runtime(
    runtime: AgentRuntimeOptions,
    selector: &str,
    backend: BackendKind,
) -> Result<ResolvedAgentRuntime, Box<dyn std::error::Error>> {
    let mut label = runtime.label.clone().unwrap_or_else(|| {
        if selector.trim().is_empty() {
            default_selector_for_backend(backend).to_string()
        } else {
            selector.to_string()
        }
    });
    let mut progress_filter = runtime.progress_filter.clone();
    let output = AgentOutputHandling::Wrapped;

    if progress_filter.is_none() {
        progress_filter = default_progress_filter_for_label(&label);
    }

    if !runtime.command.is_empty() {
        if label.is_empty() {
            label = default_selector_for_backend(backend).to_string();
        } else if runtime.label.is_none() {
            label = command_label(&runtime.command).unwrap_or(label);
        }

        return Ok(ResolvedAgentRuntime {
            label,
            command: runtime.command,
            progress_filter,
            output,
            enable_script_wrapper: runtime.enable_script_wrapper,
            resolution: AgentRuntimeResolution::ProvidedCommand,
        });
    }

    if backend.requires_agent_runner() {
        let Some(path) = bundled_agent_command(&label) else {
            let locations: Vec<String> = bundled_agent_shim_dir_candidates()
                .iter()
                .map(|p| p.display().to_string())
                .collect();
            let hint = if locations.is_empty() {
                "no shim directories detected".to_string()
            } else {
                format!("looked in {}", locations.join(", "))
            };
            return Err(Box::new(io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "no bundled agent shim named `{label}` was found ({hint}); set agent.command to a script that prints assistant output to stdout and progress/errors to stderr"
                ),
            )));
        };

        return Ok(ResolvedAgentRuntime {
            label: label.clone(),
            command: vec![path.display().to_string()],
            progress_filter,
            output,
            enable_script_wrapper: runtime.enable_script_wrapper,
            resolution: AgentRuntimeResolution::BundledShim { label, path },
        });
    }

    Ok(ResolvedAgentRuntime {
        label,
        command: Vec::new(),
        progress_filter,
        output,
        enable_script_wrapper: runtime.enable_script_wrapper,
        resolution: AgentRuntimeResolution::ProvidedCommand,
    })
}

fn resolve_agent_runner(
    backend: BackendKind,
) -> Result<Option<Arc<dyn AgentRunner>>, Box<dyn std::error::Error>> {
    if !backend.requires_agent_runner() {
        return Ok(None);
    }

    Ok(Some(Arc::new(ScriptRunner)))
}
