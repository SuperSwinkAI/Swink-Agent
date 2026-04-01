use regex::Regex;

pub fn compile_regex(pattern: &str) -> Result<Regex, regex::Error> {
    Regex::new(pattern)
}

pub fn compile_case_insensitive_regex(pattern: &str) -> Result<Regex, regex::Error> {
    compile_regex(&format!("(?i){pattern}"))
}

pub fn compile_named_regexes<T, F>(
    defs: &[(&str, &str)],
    mut build: F,
) -> Result<Vec<T>, regex::Error>
where
    F: FnMut(String, Regex) -> T,
{
    defs.iter()
        .map(|(name, pattern)| {
            compile_regex(pattern).map(|regex| build((*name).to_string(), regex))
        })
        .collect()
}
