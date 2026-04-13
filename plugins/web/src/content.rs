use std::io::Cursor;

use swink_agent::{prefix_chars, suffix_chars};

/// Errors that can occur during content extraction.
#[derive(Debug, thiserror::Error)]
pub enum ContentError {
    #[error("Readability extraction failed: {0}")]
    ExtractionFailed(String),
}

/// Result of extracting readable content from raw HTML.
///
/// Contains only data the readability extractor actually knows. HTTP metadata
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

/// Extract readable content from raw HTML bytes using the readability algorithm.
///
/// Strips navigation, ads, scripts, and boilerplate, returning the main article
/// text with an optional title.
pub fn extract_readable_content(
    html: &[u8],
    url: &url::Url,
) -> Result<FetchedContent, ContentError> {
    let mut cursor = Cursor::new(html);
    let product = readability::extractor::extract(&mut cursor, url)
        .map_err(|e| ContentError::ExtractionFailed(e.to_string()))?;

    let title = if product.title.is_empty() {
        None
    } else {
        Some(product.title)
    };

    let text_length = product.text.chars().count();

    Ok(FetchedContent {
        url: url.to_string(),
        title,
        text: product.text,
        text_length,
    })
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
        assert!(result.contains("209 chars total"));
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
                <p>This is the main article content that should be extracted by the readability algorithm. It contains enough text to be considered the primary content of the page.</p>
                <p>Here is a second paragraph with more meaningful content to help the readability algorithm identify this as the main content block of the page.</p>
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
                   has enough text for readability to pick it up as the main content block.</p>
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
                in the readability extractor output.</p>
                <p>Second paragraph to ensure readability picks this up as the main content.</p>
            </article>
        </body>
        </html>
        "#;

        let url = url::Url::parse("https://example.com/unicode").unwrap();
        let result = extract_readable_content(html, &url).unwrap();

        // text_length must equal chars().count(), not bytes len()
        assert_eq!(result.text_length, result.text.chars().count());
    }
}
