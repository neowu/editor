use std::path::PathBuf;

use anyhow::Context as _;
use rustc_hash::FxHashMap;
use serde::Deserialize;
use serde::Serialize;
use sha2::Digest as _;
use sha2::Sha256;
use util::ResultExt as _;
use util::truncate_and_remove_front;

use crate::ResolvedTask;
use crate::SpawnInTerminal;
use crate::TaskContext;
use crate::TaskId;

/// A template definition of an ed task to run.
/// May use the [`VariableName`](crate::VariableName) to get the corresponding substitutions into its fields.
///
/// Template itself is not ready to spawn a task, it needs to be resolved with a [`TaskContext`] first, that
/// contains all relevant ed state in task variables.
/// A single template may produce different tasks (or none) for different contexts.
#[derive(Clone, Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskTemplate {
    /// Human readable name of the task to display in the UI.
    pub label: String,
    /// Executable command to spawn.
    pub command: String,
    /// Arguments to the command.
    pub args: Vec<String>,
    /// Env overrides for the command, will be appended to the terminal's environment from the settings.
    pub env: FxHashMap<String, String>,
    /// Current working directory to spawn the command into, defaults to current project root.
    pub cwd: Option<String>,
    /// The runnable tags (from `runnables.scm`) this template attaches to.
    pub tags: Vec<String>,
}

/// What to do with the terminal pane and tab, after the command was started.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevealStrategy {
    /// Always show the task's pane, and focus the corresponding tab in it.
    #[default]
    Always,
    /// Always show the task's pane, add the task's tab in it, but don't focus it.
    NoFocus,
    /// Do not alter focus, but still add/reuse the task's tab in its pane.
    Never,
}

/// A group of task templates, provided by a language.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TaskTemplates(pub Vec<TaskTemplate>);

impl TaskTemplate {
    /// Replaces all `VariableName` task variables in the task template string fields.
    ///
    /// Every [`ResolvedTask`] gets a [`TaskId`], based on the `id_base` (to avoid collision with various task sources),
    /// and hashes of its template and [`TaskContext`].
    pub fn resolve_task(&self, id_base: &str, cx: &TaskContext) -> Option<ResolvedTask> {
        if self.label.trim().is_empty() || self.command.trim().is_empty() {
            return None;
        }

        let task_variables = cx
            .task_variables
            .0
            .iter()
            .map(|(key, value)| (key.to_string(), value.as_str()))
            .collect::<FxHashMap<_, _>>();
        let truncated_variables = truncate_variables(&task_variables);
        let cwd = match self.cwd.as_deref() {
            Some(cwd) => Some(PathBuf::from(substitute_all_template_variables_in_str(cwd, &task_variables)?)),
            None => None,
        }
        .or_else(|| cx.cwd.clone());
        let full_label = substitute_all_template_variables_in_str(&self.label, &task_variables)?;

        // Arbitrarily picked threshold below which we don't truncate any variables.
        const TRUNCATION_THRESHOLD: usize = 64;

        let human_readable_label = if full_label.len() > TRUNCATION_THRESHOLD {
            substitute_all_template_variables_in_str(&self.label, &truncated_variables)?
        } else {
            #[allow(
                clippy::redundant_clone,
                reason = "We want to clone the full_label to avoid borrowing it in the fold closure"
            )]
            full_label.clone()
        }
        .lines()
        .fold(String::new(), |mut string, line| {
            if !string.is_empty() {
                string.push_str("\\n");
            }
            string.push_str(line);
            string
        });

        let command = substitute_all_template_variables_in_str(&self.command, &task_variables)?;
        let args_with_substitutions = substitute_all_template_variables_in_vec(&self.args, &task_variables)?;

        let task_hash = to_hex_hash(self).context("hashing task template").log_err()?;
        let variables_hash = to_hex_hash(&task_variables).context("hashing task variables").log_err()?;
        let id = TaskId(format!("{id_base}_{task_hash}_{variables_hash}"));

        let env = {
            // The project environment is the base; only the template's own values hold variables.
            let mut env = cx.project_env.clone();
            for (key, value) in &self.env {
                env.insert(key.clone(), substitute_all_template_variables_in_str(value, &task_variables)?);
            }
            env
        };

        Some(ResolvedTask {
            id: id.clone(),
            original_task: self.clone(),
            resolved_label: full_label.clone(),
            resolved: SpawnInTerminal {
                id,
                cwd,
                full_label,
                label: human_readable_label,
                command_label: args_with_substitutions.iter().fold(command.clone(), |mut command_label, arg| {
                    command_label.push(' ');
                    command_label.push_str(arg);
                    command_label
                }),
                command: Some(command),
                args: args_with_substitutions,
                env,
                ..SpawnInTerminal::default()
            },
        })
    }
}

const MAX_DISPLAY_VARIABLE_LENGTH: usize = 15;

fn truncate_variables(task_variables: &FxHashMap<String, &str>) -> FxHashMap<String, String> {
    task_variables
        .iter()
        .map(|(key, value)| (key.clone(), truncate_and_remove_front(value, MAX_DISPLAY_VARIABLE_LENGTH)))
        .collect()
}

fn to_hex_hash(object: impl Serialize) -> anyhow::Result<String> {
    let json = serde_json::to_string(&object).context("serializing the object")?;
    let mut hasher = Sha256::new();
    hasher.update(json.as_bytes());
    Ok(to_hex(&hasher.finalize()))
}

fn to_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut hex = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        hex.push(DIGITS[usize::from(byte >> 4)] as char);
        hex.push(DIGITS[usize::from(byte & 0xf)] as char);
    }
    hex
}

/// Replaces every `$ED_NAME` with its task variable, failing the template if
/// one is unset. Templates are built in code and only use this form, so
/// nothing else is interpreted: any other `$` reaches the shell as written.
fn substitute_all_template_variables_in_str<A: AsRef<str>>(
    template_str: &str,
    task_variables: &FxHashMap<String, A>,
) -> Option<String> {
    let mut result = String::with_capacity(template_str.len());
    let mut rest = template_str;
    while let Some(start) = rest.find("$ED_") {
        result.push_str(&rest[..start]);
        let name = &rest[start + 1..];
        let len = name.find(|c: char| !c.is_ascii_alphanumeric() && c != '_').unwrap_or(name.len());
        result.push_str(task_variables.get(&name[..len])?.as_ref());
        rest = &name[len..];
    }
    result.push_str(rest);
    Some(result)
}

fn substitute_all_template_variables_in_vec(
    template_strs: &[String],
    task_variables: &FxHashMap<String, &str>,
) -> Option<Vec<String>> {
    let mut expanded = Vec::with_capacity(template_strs.len());
    for variable in template_strs {
        let new_value = substitute_all_template_variables_in_str(variable, task_variables)?;

        if !new_value.is_empty() || variable.is_empty() {
            expanded.push(new_value);
        }
    }

    Some(expanded)
}

#[cfg(test)]
mod tests {

    use rustc_hash::FxHashMap;

    use super::TaskContext;
    use super::TaskTemplate;
    use super::substitute_all_template_variables_in_str;
    use super::to_hex;
    use crate::TaskVariables;
    use crate::VariableName;

    #[test]
    fn variable_names_print_with_ed_prefix() {
        use std::borrow::Cow;

        use crate::VariableName;
        assert_eq!(VariableName::Dirname.template_value(), "$ED_DIRNAME");
        assert_eq!(VariableName::Custom(Cow::Borrowed("RUST_PACKAGE")).template_value(), "$ED_RUST_PACKAGE");
    }
    #[test]
    fn substitutes_template_variables() {
        let variables = FxHashMap::from_iter([("ED_FILE".to_owned(), "/a/b.rs"), ("ED_row".to_owned(), "7")]);
        let substitute = |template: &str| substitute_all_template_variables_in_str(template, &variables);

        assert_eq!(substitute("cat $ED_FILE:$ED_row").unwrap(), "cat /a/b.rs:7");
        assert_eq!(substitute("é\\^$ED_row\\$é").unwrap(), "é\\^7\\$é");
        assert_eq!(substitute("$ED_NOPE"), None);
        assert_eq!(substitute("$ED_"), None);
        // Nothing but `$ED_*` is interpreted.
        for literal in ["", "$PATH ${HOME:-/x} $(pwd) $$ $1 $", "${ED_FILE}", "$EDITOR"] {
            assert_eq!(substitute(literal).as_deref(), Some(literal));
        }
    }

    #[test]
    fn resolve_task_substitutes_template_env_only() {
        let template = TaskTemplate {
            label: "run".into(),
            command: "echo".into(),
            env: FxHashMap::from_iter([("CODE".to_owned(), "$ED_FILE".to_owned())]),
            ..TaskTemplate::default()
        };
        let context = TaskContext {
            task_variables: TaskVariables::from_iter([(VariableName::File, "/a/b.rs".to_owned())]),
            project_env: FxHashMap::from_iter([
                ("CODE".to_owned(), "overridden".to_owned()),
                ("PS1".to_owned(), "$ED_NOPE $$".to_owned()),
            ]),
            ..TaskContext::default()
        };
        let env = template.resolve_task("test", &context).unwrap().resolved.env;
        assert_eq!(env["CODE"], "/a/b.rs");
        assert_eq!(env["PS1"], "$ED_NOPE $$");
        // Task variables are substituted, never exported.
        assert_eq!(env.len(), 2);
    }

    #[test]
    fn to_hex_encodes_lowercase_pairs() {
        assert_eq!(to_hex(&[]), "");
        assert_eq!(to_hex(&[0x00, 0x0f, 0xa5, 0xff]), "000fa5ff");
    }

    #[test]
    fn resolve_task_preserves_multiline_labels_and_escapes_display_newlines() {
        let template = TaskTemplate { label: "Build\n\nRun".into(), command: "echo".into(), ..TaskTemplate::default() };
        let task = template.resolve_task("test", &TaskContext::default()).unwrap();
        assert_eq!(task.resolved_label, "Build\n\nRun");
        assert_eq!(task.resolved.full_label, "Build\n\nRun");
        assert_eq!(task.resolved.label, "Build\\n\\nRun");

        let template = TaskTemplate { label: "Build".into(), ..template };
        let task = template.resolve_task("test", &TaskContext::default()).unwrap();
        assert_eq!(task.resolved.label, "Build");
    }
}
