use scraper::{Html, Selector};

use super::{SearchError, SearchProvider, SearchResult};

/// DuckDuckGo Lite HTML scraping provider.
///
/// Scrapes results from `lite.duckduckgo.com` — no API key required.
#[derive(Debug)]
pub struct DuckDuckGoProvider {
    pub(crate) client: reqwest::Client,
}

impl DuckDuckGoProvider {
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }

    /// Parse DDG Lite HTML into search results.
    ///
    /// Public for testing with fixture HTML.
    pub fn parse_results(html: &str, max_results: usize) -> Vec<SearchResult> {
        let document = Html::parse_document(html);

        // DDG Lite renders results in a table. Each result has:
        //   - An <a class="result-link"> with href and title text
        //   - A <td class="result-snippet"> with the snippet
        //
        // We try the canonical selectors first, then fall back to
        // broader heuristics if the page structure differs.
        let link_sel = Selector::parse("a.result-link").ok();
        let snippet_sel = Selector::parse("td.result-snippet, .result-snippet").ok();

        if let (Some(l_sel), Some(s_sel)) = (&link_sel, &snippet_sel) {
            let links: Vec<_> = document.select(l_sel).collect();
            let snippets: Vec<_> = document.select(s_sel).collect();

            if !links.is_empty() {
                let mut results = Vec::with_capacity(max_results);
                for (i, link_el) in links.into_iter().enumerate() {
                    if results.len() >= max_results {
                        break;
                    }
                    let title = link_el.text().collect::<String>().trim().to_owned();
                    let url = link_el.value().attr("href").unwrap_or_default().to_owned();
                    let snippet = snippets
                        .get(i)
                        .map(|el| el.text().collect::<String>().trim().to_owned())
                        .unwrap_or_default();

                    if !url.is_empty() {
                        results.push(SearchResult {
                            title,
                            url,
                            snippet,
                        });
                    }
                }
                return results;
            }
        }

        // Fallback: look for links inside table rows that have an href
        // starting with "http". This covers layout variations.
        let row_sel = Selector::parse("table tr").ok();
        let a_sel = Selector::parse("a[href]").ok();

        if let (Some(r_sel), Some(a_sel_inner)) = (&row_sel, &a_sel) {
            let mut results = Vec::with_capacity(max_results);
            for row in document.select(r_sel) {
                if results.len() >= max_results {
                    break;
                }
                if let Some(a_el) = row.select(a_sel_inner).next() {
                    let href = a_el.value().attr("href").unwrap_or_default();
                    if href.starts_with("http") {
                        let title = a_el.text().collect::<String>().trim().to_owned();
                        // Use the remaining row text (minus the link text) as snippet.
                        let full_text = row.text().collect::<String>();
                        let snippet = full_text.replace(&title, "").trim().to_owned();
                        results.push(SearchResult {
                            title,
                            url: href.to_owned(),
                            snippet,
                        });
                    }
                }
            }
            return results;
        }

        Vec::new()
    }
}

impl SearchProvider for DuckDuckGoProvider {
    fn name(&self) -> &str {
        "duckduckgo"
    }

    fn search(
        &self,
        query: &str,
        max_results: usize,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Vec<SearchResult>, SearchError>> + Send + '_>,
    > {
        let query = query.to_owned();
        Box::pin(async move {
            let response = self
                .client
                .post("https://lite.duckduckgo.com/lite/")
                .form(&[("q", &query)])
                .send()
                .await
                .map_err(|e| SearchError::NetworkError(e.to_string()))?;

            if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
                return Err(SearchError::RateLimited);
            }

            if !response.status().is_success() {
                return Err(SearchError::ProviderUnavailable(format!(
                    "DuckDuckGo returned status {}",
                    response.status()
                )));
            }

            let html = response
                .text()
                .await
                .map_err(|e| SearchError::ParseError(e.to_string()))?;

            Ok(Self::parse_results(&html, max_results))
        })
    }
}
