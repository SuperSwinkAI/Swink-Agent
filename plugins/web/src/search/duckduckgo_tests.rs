//! Tests for `duckduckgo`.
#![cfg(test)]

use super::DuckDuckGoProvider;

const FIXTURE_HTML: &str = r#"
    <html><body>
    <table>
      <tr>
        <td><a class="result-link" href="https://example.com/one">First Result</a></td>
        <td class="result-snippet">This is the first snippet.</td>
      </tr>
      <tr>
        <td><a class="result-link" href="https://example.com/two">Second Result</a></td>
        <td class="result-snippet">This is the second snippet.</td>
      </tr>
    </table>
    </body></html>
    "#;

#[test]
fn parses_result_links_and_snippets() {
    let results = DuckDuckGoProvider::parse_results(FIXTURE_HTML, 10);
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].title, "First Result");
    assert_eq!(results[0].url, "https://example.com/one");
    assert_eq!(results[0].snippet, "This is the first snippet.");
}

#[test]
fn fallback_parsing_for_plain_table_links() {
    let html = r#"
        <html><body>
        <table>
          <tr>
            <td><a href="https://fallback.example.com">Fallback Title</a></td>
            <td>Some extra text for snippet</td>
          </tr>
        </table>
        </body></html>
        "#;
    let results = DuckDuckGoProvider::parse_results(html, 10);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].url, "https://fallback.example.com");
    assert_eq!(results[0].title, "Fallback Title");
}
