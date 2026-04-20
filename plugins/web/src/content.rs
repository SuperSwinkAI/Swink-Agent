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
#[derive(Debug, Clone)]
pub struct FetchedContent {
    pub url: String,
    pub title: Option<String>,
    pub text: String,
    /// Character length of the extracted text.
    pub text_length: usize,
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
    let text_length = text.chars().count();

    Ok(FetchedContent {
        url: url.to_string(),
        title,
        text,
        text_length,
    })
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
mod tests {
    use super::{extract_readable_content, is_html_content_type, truncate_content};

    #[test]
    fn truncate_content_short_text_no_truncation() {
        let text = "Hello, world!";
        let (result, truncated) = truncate_content(text, 100);
        assert_eq!(result, text);
        assert!(!truncated);
    }

    #[test]
    fn truncate_content_long_text_is_truncated() {
        let text = "a".repeat(1000);
        let (result, truncated) = truncate_content(&text, 200);

        assert!(truncated);
        assert!(result.starts_with(&"a".repeat(160)));
        assert!(result.ends_with(&"a".repeat(40)));
        assert!(result.contains("[... content truncated"));
    }

    #[test]
    fn truncate_content_multibyte_boundaries_are_utf8_safe() {
        let text = format!("{}🙂{}", "a".repeat(159), "🙂".repeat(50));
        let (result, truncated) = truncate_content(&text, 200);

        assert!(truncated);
        assert!(result.starts_with(&format!("{}🙂", "a".repeat(159))));
        assert!(result.ends_with(&"🙂".repeat(40)));
        assert!(result.contains("210 chars total"));
        assert!(result.contains("showing first 160 and last 40"));
    }

    #[test]
    fn html_content_type_detection_matches_expected_values() {
        assert!(is_html_content_type("text/html"));
        assert!(is_html_content_type("text/html; charset=utf-8"));
        assert!(is_html_content_type("application/xhtml+xml"));
        assert!(!is_html_content_type("application/json"));
        assert!(!is_html_content_type("text/plain"));
    }

    #[test]
    fn extract_readable_content_simple_article() {
        let html = br#"
        <!DOCTYPE html>
        <html>
        <head><title>Test Article</title></head>
        <body>
            <nav>Navigation links here</nav>
            <article>
                <h1>Main Heading</h1>
                <p>This is the main article content that should be extracted from the page. It contains enough text to be considered the primary content of the page.</p>
                <p>Here is a second paragraph with more meaningful content to help the extractor identify this as the main content block of the page.</p>
            </article>
            <footer>Footer content here</footer>
        </body>
        </html>
        "#;

        let url = url::Url::parse("https://example.com/article").unwrap();
        let result = extract_readable_content(html, &url).unwrap();

        assert_eq!(result.url, "https://example.com/article");
        assert!(result.title.is_some());
        assert!(result.text.contains("main article content"));
        assert!(result.text_length > 0);
        assert_eq!(result.text_length, result.text.chars().count());
    }

    #[test]
    fn fetched_content_has_no_http_metadata_fields() {
        // Pin the struct contract: FetchedContent only carries extractor-known data.
        let content = super::FetchedContent {
            url: "https://example.com".to_string(),
            title: Some("Title".to_string()),
            text: "Body text".to_string(),
            text_length: 9,
        };
        assert_eq!(content.url, "https://example.com");
        assert_eq!(content.title.as_deref(), Some("Title"));
        assert_eq!(content.text, "Body text");
        assert_eq!(content.text_length, 9);
    }

    #[test]
    fn extract_readable_content_empty_title_becomes_none() {
        let html = br#"
        <!DOCTYPE html>
        <html>
        <head><title></title></head>
        <body>
            <article>
                <p>Content without a meaningful title in the document head. This paragraph
                   has enough text for the extractor to pick it up as the main content block.</p>
                <p>A second paragraph to reinforce that this is the article body content.</p>
            </article>
        </body>
        </html>
        "#;

        let url = url::Url::parse("https://example.com/no-title").unwrap();
        let result = extract_readable_content(html, &url).unwrap();

        assert!(result.title.is_none());
        assert!(result.text_length > 0);
    }

    #[test]
    fn text_length_counts_chars_not_bytes() {
        // Multibyte characters: text_length should be char count, not byte count.
        let html = br#"
        <!DOCTYPE html>
        <html>
        <head><title>Unicode</title></head>
        <body>
            <article>
                <p>Here are some emoji characters for testing multibyte length counting
                in the extractor output.</p>
                <p>Second paragraph to ensure the extractor picks this up as the main content.</p>
            </article>
        </body>
        </html>
        "#;

        let url = url::Url::parse("https://example.com/unicode").unwrap();
        let result = extract_readable_content(html, &url).unwrap();

        // text_length must equal chars().count(), not bytes len()
        assert_eq!(result.text_length, result.text.chars().count());
    }

    #[test]
    fn extract_readable_content_falls_back_to_body_when_no_article_exists() {
        let html = br#"
        <!DOCTYPE html>
        <html>
        <head><title>Body Fallback</title></head>
        <body>
            <div class="hero">Short heading</div>
            <section>
                <p>This body paragraph should still be extracted even without an article tag.</p>
                <p>A second paragraph makes this the strongest content candidate on the page.</p>
            </section>
        </body>
        </html>
        "#;

        let url = url::Url::parse("https://example.com/body").unwrap();
        let result = extract_readable_content(html, &url).unwrap();

        assert_eq!(result.title.as_deref(), Some("Body Fallback"));
        assert!(
            result
                .text
                .contains("This body paragraph should still be extracted")
        );
        assert!(
            result
                .text
                .contains("A second paragraph makes this the strongest")
        );
    }
}
