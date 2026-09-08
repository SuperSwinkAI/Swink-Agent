use scraper::{ElementRef, Html, Selector};
use swink_agent::{prefix_chars, suffix_chars};

/// Errors that can occur during content extraction.
#[derive(Debug, thiserror::Error)]
pub enum ContentError {
    #[error("content extraction failed: {0}")]
    ExtractionFailed(String),
}

/// Result of extracting readable content from raw HTML.
///
/// Contains only data the extractor actually knows. HTTP metadata
/// (status code, content type, truncation) belongs in the caller that holds the
/// real HTTP response.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct FetchedContent {
    pub url: String,
    pub title: Option<String>,
    pub text: String,
    /// Character length of the extracted text.
    pub text_length: usize,
}

impl FetchedContent {
    /// Construct from extracted fields, computing `text_length` from `text`.
    #[must_use]
    pub fn new(url: impl Into<String>, title: Option<String>, text: impl Into<String>) -> Self {
        let text = text.into();
        let text_length = text.chars().count();
        Self {
            url: url.into(),
            title,
            text,
            text_length,
        }
    }
}

/// Extract readable content from raw HTML bytes.
///
/// Prefers article-like containers and falls back to the page body, then
/// flattens common text-bearing block elements into plain text.
pub fn extract_readable_content(
    html: &[u8],
    url: &url::Url,
) -> Result<FetchedContent, ContentError> {
    let html = String::from_utf8_lossy(html);
    let document = Html::parse_document(&html);

    let title = extract_title(&document);
    let text = extract_main_text(&document).ok_or_else(|| {
        ContentError::ExtractionFailed("no readable text blocks found".to_string())
    })?;

    Ok(FetchedContent::new(url.to_string(), title, text))
}

fn extract_title(document: &Html) -> Option<String> {
    let selector = Selector::parse("title").expect("valid selector");
    document
        .select(&selector)
        .next()
        .map(element_text)
        .filter(|title| !title.is_empty())
}

fn extract_main_text(document: &Html) -> Option<String> {
    let candidate_selector = Selector::parse(
        "article, main, [role='main'], .article, .post, .entry-content, .content, section, body",
    )
    .expect("valid selector");
    let block_selector =
        Selector::parse("h1, h2, h3, h4, h5, h6, p, li, blockquote, pre").expect("valid selector");

    let mut best: Option<String> = None;
    let mut best_score = 0usize;

    for candidate in document.select(&candidate_selector) {
        let text = collect_block_text(candidate, &block_selector);
        let score = text.chars().count();
        if score > best_score {
            best_score = score;
            best = Some(text);
        }
    }

    best.filter(|text| !text.is_empty())
}

fn collect_block_text(root: ElementRef<'_>, block_selector: &Selector) -> String {
    let mut blocks = Vec::new();

    for block in root.select(block_selector) {
        let text = element_text(block);
        if !text.is_empty() {
            blocks.push(text);
        }
    }

    if blocks.is_empty() {
        return element_text(root);
    }

    blocks.join("\n\n")
}

fn element_text(element: ElementRef<'_>) -> String {
    normalize_text(&element.text().collect::<Vec<_>>().join(" "))
}

fn normalize_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Truncate content to fit within `max_len` characters.
///
/// If the text is already within the limit, returns it unchanged with `false`.
/// Otherwise, keeps 80% from the beginning and 20% from the end, inserting a
/// truncation notice in the middle.
pub fn truncate_content(text: &str, max_len: usize) -> (String, bool) {
    let original_len = text.chars().count();
    if original_len <= max_len {
        return (text.to_string(), false);
    }

    let head_len = max_len * 80 / 100;
    let tail_len = max_len * 20 / 100;

    let head = prefix_chars(text, head_len);
    let tail = suffix_chars(text, tail_len);

    let notice = format!(
        "\n\n[... content truncated ({original_len} chars total, \
         showing first {head_len} and last {tail_len}) ...]\n\n"
    );

    let mut result = String::with_capacity(head.len() + notice.len() + tail.len());
    result.push_str(head);
    result.push_str(&notice);
    result.push_str(tail);

    (result, true)
}

/// Check whether a Content-Type header value indicates HTML content.
pub fn is_html_content_type(content_type: &str) -> bool {
    content_type.contains("text/html") || content_type.contains("application/xhtml")
}

#[cfg(test)]
#[path = "content_tests.rs"]
mod tests;
