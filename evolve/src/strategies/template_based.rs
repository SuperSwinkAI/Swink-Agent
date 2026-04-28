use crate::mutate::{Candidate, MutationContext, MutationError, MutationStrategy, deduplicate};
use rand::SeedableRng;
use rand::seq::SliceRandom;
use regex::Regex;

/// Pairs of (compiled pattern, replacement string).
struct Template {
    pattern: Regex,
    replacement: String,
}

// Ten built-in find/replace templates covering common prompt phrasing variations.
fn built_in_templates() -> Vec<Template> {
    let raw: &[(&str, &str)] = &[
        (r"\bMust\b", "Should"),
        (r"\bShould\b", "Must"),
        (r"\bIn order to\b", "To"),
        (r"\bdue to the fact that\b", "because"),
        (r"\bUtilize\b", "Use"),
        (r"\butilize\b", "use"),
        (r"\bAssist\b", "Help"),
        (r"\bassist\b", "help"),
        (r"\bSubsequently\b", "Then"),
        (r"\bsubsequently\b", "then"),
    ];
    raw.iter()
        .filter_map(|(find, replace)| {
            Regex::new(find).ok().map(|p| Template {
                pattern: p,
                replacement: replace.to_string(),
            })
        })
        .collect()
}

/// Mutation strategy that applies find-replace templates to the target text.
pub struct TemplateBased {
    templates: Vec<Template>,
}

impl TemplateBased {
    pub fn new() -> Self {
        Self {
            templates: built_in_templates(),
        }
    }

    /// Add a user-provided regex template.
    pub fn with_template(mut self, find: &str, replace: &str) -> Result<Self, regex::Error> {
        self.templates.push(Template {
            pattern: Regex::new(find)?,
            replacement: replace.to_string(),
        });
        Ok(self)
    }
}

impl Default for TemplateBased {
    fn default() -> Self {
        Self::new()
    }
}

impl MutationStrategy for TemplateBased {
    fn name(&self) -> &str {
        "template_based"
    }

    fn mutate(
        &self,
        target: &str,
        context: &MutationContext,
    ) -> Result<Vec<Candidate>, MutationError> {
        let component = context.weak_point.component.clone();
        let mut candidates: Vec<Candidate> = self
            .templates
            .iter()
            .filter_map(|t| {
                let result = t
                    .pattern
                    .replace_all(target, t.replacement.as_str())
                    .into_owned();
                if result == target {
                    None
                } else {
                    Some(Candidate::new(
                        component.clone(),
                        target.to_string(),
                        result,
                        "template_based".to_string(),
                    ))
                }
            })
            .collect();

        candidates = deduplicate(candidates, target);

        if let Some(seed) = context.seed {
            let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
            candidates.shuffle(&mut rng);
        }
        candidates.truncate(context.max_candidates);
        Ok(candidates)
    }
}
