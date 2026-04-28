use crate::mutate::MutationStrategy;
use regex::Regex;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Mutex;
use swink_agent::{Cost, ToolSchema};
use swink_agent_eval::EvalSet;

/// A named region within a system prompt, identified by section header.
#[derive(Debug, Clone, PartialEq)]
pub struct PromptSection {
    pub name: Option<String>,
    pub content: String,
    pub byte_range: Range<usize>,
}

/// The mutable components of an agent configuration: system prompt and tool schemas.
pub struct OptimizationTarget {
    system_prompt: String,
    sections: Vec<PromptSection>,
    tool_schemas: Vec<ToolSchema>,
    section_delimiter: Option<Regex>,
}

impl Clone for OptimizationTarget {
    fn clone(&self) -> Self {
        Self {
            system_prompt: self.system_prompt.clone(),
            sections: self.sections.clone(),
            tool_schemas: self
                .tool_schemas
                .iter()
                .map(|t| ToolSchema {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    parameters: t.parameters.clone(),
                })
                .collect(),
            section_delimiter: self.section_delimiter.clone(),
        }
    }
}

impl OptimizationTarget {
    /// Create from a system prompt and tool schemas. Sections auto-parsed from markdown headers.
    pub fn new(system_prompt: impl Into<String>, tool_schemas: Vec<ToolSchema>) -> Self {
        let system_prompt = system_prompt.into();
        let sections = Self::parse_sections(&system_prompt, None);
        Self {
            system_prompt,
            sections,
            tool_schemas,
            section_delimiter: None,
        }
    }

    /// Override the section delimiter regex (default: markdown `## ` headers).
    pub fn with_section_delimiter(mut self, delimiter: Regex) -> Self {
        self.sections = Self::parse_sections(&self.system_prompt, Some(&delimiter));
        self.section_delimiter = Some(delimiter);
        self
    }

    pub fn sections(&self) -> &[PromptSection] {
        &self.sections
    }

    pub fn system_prompt(&self) -> &str {
        &self.system_prompt
    }

    pub fn tool_schemas(&self) -> &[ToolSchema] {
        &self.tool_schemas
    }

    /// Produce a new target with the system prompt replaced entirely.
    pub fn with_system_prompt(&self, new_prompt: impl Into<String>) -> Self {
        let system_prompt = new_prompt.into();
        let new_delim = self.section_delimiter.clone();
        let sections = Self::parse_sections(&system_prompt, new_delim.as_ref());
        Self {
            system_prompt,
            sections,
            tool_schemas: self
                .tool_schemas
                .iter()
                .map(|t| ToolSchema {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    parameters: t.parameters.clone(),
                })
                .collect(),
            section_delimiter: new_delim,
        }
    }

    /// Produce a new target with section at `index` replaced by `new_content`.
    pub fn with_replaced_section(&self, index: usize, new_content: &str) -> Self {
        let old = &self.sections[index];
        let new_prompt = format!(
            "{}{}{}",
            &self.system_prompt[..old.byte_range.start],
            new_content,
            &self.system_prompt[old.byte_range.end..],
        );
        let new_delim = self.section_delimiter.clone();
        let sections = Self::parse_sections(&new_prompt, new_delim.as_ref());
        Self {
            system_prompt: new_prompt,
            sections,
            tool_schemas: self
                .tool_schemas
                .iter()
                .map(|t| ToolSchema {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    parameters: t.parameters.clone(),
                })
                .collect(),
            section_delimiter: new_delim,
        }
    }

    /// Produce a new target with the named tool schema replaced.
    pub fn with_replaced_tool(&self, tool_name: &str, schema: ToolSchema) -> Self {
        let mut replacement = Some(schema);
        let tool_schemas = self
            .tool_schemas
            .iter()
            .map(|t| {
                if t.name == tool_name {
                    replacement.take().unwrap_or_else(|| ToolSchema {
                        name: t.name.clone(),
                        description: t.description.clone(),
                        parameters: t.parameters.clone(),
                    })
                } else {
                    ToolSchema {
                        name: t.name.clone(),
                        description: t.description.clone(),
                        parameters: t.parameters.clone(),
                    }
                }
            })
            .collect();
        Self {
            system_prompt: self.system_prompt.clone(),
            sections: self.sections.clone(),
            tool_schemas,
            section_delimiter: self.section_delimiter.clone(),
        }
    }

    fn parse_sections(text: &str, delimiter: Option<&Regex>) -> Vec<PromptSection> {
        let default_re = Regex::new(r"(?m)^## (.+)$").unwrap();
        let re = delimiter.unwrap_or(&default_re);

        let positions: Vec<(usize, Option<String>)> = re
            .captures_iter(text)
            .map(|cap| {
                let start = cap.get(0).unwrap().start();
                let end = cap.get(0).unwrap().end();
                let name = cap.get(1).map(|n| n.as_str().trim().to_string());
                let _ = end;
                (start, name)
            })
            .collect();

        if positions.is_empty() {
            return vec![PromptSection {
                name: None,
                content: text.to_string(),
                byte_range: 0..text.len(),
            }];
        }

        positions
            .iter()
            .enumerate()
            .map(|(i, (start, name))| {
                let end = positions.get(i + 1).map(|(s, _)| *s).unwrap_or(text.len());
                PromptSection {
                    name: name.clone(),
                    content: text[*start..end].to_string(),
                    byte_range: *start..end,
                }
            })
            .collect()
    }
}

/// Per-cycle cost accumulator. Shared via `Arc<CycleBudget>` across all phases.
pub struct CycleBudget {
    max_cost: Cost,
    spent: Mutex<Cost>,
}

impl CycleBudget {
    pub fn new(max_cost: Cost) -> Self {
        Self {
            max_cost,
            spent: Mutex::new(Cost::default()),
        }
    }

    pub fn record(&self, cost: Cost) {
        let mut spent = self.spent.lock().unwrap();
        *spent += cost;
    }

    pub fn remaining(&self) -> Cost {
        let spent = self.spent.lock().unwrap();
        Cost {
            total: (self.max_cost.total - spent.total).max(0.0),
            ..Cost::default()
        }
    }

    pub fn is_exhausted(&self) -> bool {
        let spent = self.spent.lock().unwrap();
        spent.total >= self.max_cost.total
    }
}

/// Configuration for an optimization run.
pub struct OptimizationConfig {
    pub eval_set: EvalSet,
    pub strategies: Vec<Box<dyn MutationStrategy>>,
    pub acceptance_threshold: f64,
    pub budget: CycleBudget,
    pub parallelism: usize,
    pub seed: Option<u64>,
    pub max_weak_points: usize,
    pub max_candidates_per_strategy: usize,
    pub output_root: PathBuf,
}

impl OptimizationConfig {
    pub fn new(eval_set: EvalSet, output_root: impl Into<PathBuf>) -> Self {
        Self {
            eval_set,
            strategies: Vec::new(),
            acceptance_threshold: 0.01,
            // Unlimited budget by default — callers set an explicit cap via with_budget.
            budget: CycleBudget::new(Cost {
                total: f64::INFINITY,
                ..Cost::default()
            }),
            parallelism: 1,
            seed: None,
            max_weak_points: 5,
            max_candidates_per_strategy: 3,
            output_root: output_root.into(),
        }
    }

    pub fn with_strategies(mut self, strategies: Vec<Box<dyn MutationStrategy>>) -> Self {
        self.strategies = strategies;
        self
    }

    pub fn with_acceptance_threshold(mut self, threshold: f64) -> Self {
        self.acceptance_threshold = threshold;
        self
    }

    pub fn with_budget(mut self, budget: CycleBudget) -> Self {
        self.budget = budget;
        self
    }

    pub fn with_parallelism(mut self, parallelism: usize) -> Self {
        self.parallelism = parallelism;
        self
    }

    pub fn with_seed(mut self, seed: u64) -> Self {
        self.seed = Some(seed);
        self
    }

    pub fn with_max_weak_points(mut self, max: usize) -> Self {
        self.max_weak_points = max;
        self
    }

    pub fn with_max_candidates_per_strategy(mut self, max: usize) -> Self {
        self.max_candidates_per_strategy = max;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structured_prompt_parsed_into_sections() {
        let prompt =
            "## Persona\nYou are helpful.\n\n## Rules\nBe concise.\n\n## Constraints\nNo markdown.";
        let target = OptimizationTarget::new(prompt, vec![]);
        assert_eq!(target.sections().len(), 3);
        assert_eq!(target.sections()[0].name.as_deref(), Some("Persona"));
        assert_eq!(target.sections()[1].name.as_deref(), Some("Rules"));
        assert_eq!(target.sections()[2].name.as_deref(), Some("Constraints"));
    }

    #[test]
    fn unstructured_prompt_is_one_unnamed_section() {
        let prompt = "You are a helpful assistant that answers questions.";
        let target = OptimizationTarget::new(prompt, vec![]);
        assert_eq!(target.sections().len(), 1);
        assert_eq!(target.sections()[0].name, None);
        assert_eq!(target.sections()[0].content, prompt);
        assert_eq!(target.sections()[0].byte_range, 0..prompt.len());
    }

    #[test]
    fn custom_delimiter_overrides_default() {
        let prompt = "### Alpha\nFirst content\n### Beta\nSecond content";
        let delim = Regex::new(r"(?m)^### (.+)$").unwrap();
        let target = OptimizationTarget::new(prompt, vec![]).with_section_delimiter(delim);
        assert_eq!(target.sections().len(), 2);
        assert_eq!(target.sections()[0].name.as_deref(), Some("Alpha"));
        assert_eq!(target.sections()[1].name.as_deref(), Some("Beta"));
    }

    #[test]
    fn budget_tracks_spending() {
        let budget = CycleBudget::new(Cost {
            total: 1.0,
            ..Cost::default()
        });
        budget.record(Cost {
            total: 0.3,
            ..Cost::default()
        });
        budget.record(Cost {
            total: 0.3,
            ..Cost::default()
        });
        assert!(!budget.is_exhausted());
        budget.record(Cost {
            total: 0.5,
            ..Cost::default()
        });
        assert!(budget.is_exhausted());
    }

    #[test]
    fn budget_exhausted_at_max() {
        let budget = CycleBudget::new(Cost {
            total: 1.0,
            ..Cost::default()
        });
        budget.record(Cost {
            total: 1.0,
            ..Cost::default()
        });
        assert!(budget.is_exhausted());
    }
}
