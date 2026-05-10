use regex::{Captures, Regex};
use url::Url;

#[derive(Debug, Clone)]
pub struct ExtractOptions {
    pub strategy: String,
    pub include_images: bool,
    pub include_links: bool,
    pub include_tables: bool,
    pub max_table_rows: usize,
    pub max_table_cols: usize,
    pub preserve_code_blocks: bool,
    pub strip_tracking_params: bool,
    pub min_extracted_chars: usize,
    pub dedupe_boilerplate: bool,
}

impl Default for ExtractOptions {
    fn default() -> Self {
        Self {
            strategy: "auto".to_string(),
            include_images: false,
            include_links: true,
            include_tables: true,
            max_table_rows: 20,
            max_table_cols: 6,
            preserve_code_blocks: true,
            strip_tracking_params: true,
            min_extracted_chars: 400,
            dedupe_boilerplate: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ExtractedContent {
    pub title: Option<String>,
    pub markdown: String,
    pub method: String,
    pub status: String,
    pub visible_chars: usize,
}

pub fn extract_markdown(html: &str) -> (Option<String>, String) {
    let extracted = extract_markdown_with_options(html, &ExtractOptions::default());
    (extracted.title, extracted.markdown)
}

pub fn extract_markdown_with_options(html: &str, options: &ExtractOptions) -> ExtractedContent {
    let title = re(r"(?is)<title>(.*?)</title>")
        .captures(html)
        .and_then(|c| c.get(1).map(|m| html_unescape(m.as_str())));

    let mut working = html.to_string();
    working = re(r"(?is)<!--.*?-->")
        .replace_all(&working, " ")
        .to_string();
    working = re(r"(?is)<(?:script|style|noscript)[^>]*>.*?</(?:script|style|noscript)>")
        .replace_all(&working, " ")
        .to_string();

    if !options.include_images {
        working = re(r"(?is)<(img|picture|source|svg|canvas)[^>]*?/?>")
            .replace_all(&working, " ")
            .to_string();
    }

    let (selected, method) = select_content_by_strategy(&working, options);
    let mut md = html_to_markdown(&selected, options);
    md = if options.dedupe_boilerplate {
        normalize_markdown(&md)
    } else {
        normalize_spacing(&md)
    };

    let visible_chars = md.chars().count();
    let status = extraction_status_for(html, visible_chars, options.min_extracted_chars);

    ExtractedContent {
        title,
        markdown: md,
        method,
        status,
        visible_chars,
    }
}

fn select_content_by_strategy(html: &str, options: &ExtractOptions) -> (String, String) {
    let min_chars = options.min_extracted_chars.max(120);
    match options.strategy.as_str() {
        "clean_full_page" => (html.to_string(), "clean_full_page".to_string()),
        "semantic_selector" => {
            if let Some(selected) = select_main_content(html, min_chars) {
                (selected, "semantic_selector".to_string())
            } else {
                (html.to_string(), "semantic_selector_fallback".to_string())
            }
        }
        "readability" => {
            if let Some(block) = pick_largest_block(html) {
                (block, "readability_largest_block".to_string())
            } else {
                (html.to_string(), "readability_fallback".to_string())
            }
        }
        _ => {
            if let Some(selected) = select_main_content(html, min_chars) {
                (selected, "auto_semantic".to_string())
            } else if let Some(block) = pick_largest_block(html) {
                (block, "auto_largest_block".to_string())
            } else {
                (html.to_string(), "auto_full_page".to_string())
            }
        }
    }
}

fn extraction_status_for(raw_html: &str, extracted_chars: usize, min_chars: usize) -> String {
    let threshold = min_chars.max(80);
    if extracted_chars >= threshold {
        return "ok".to_string();
    }

    if is_dynamic_content_likely(raw_html, extracted_chars, threshold) {
        "dynamic_content_unavailable".to_string()
    } else {
        "insufficient_content".to_string()
    }
}

fn is_dynamic_content_likely(raw_html: &str, extracted_chars: usize, min_chars: usize) -> bool {
    if extracted_chars >= min_chars {
        return false;
    }

    let lower = raw_html.to_ascii_lowercase();
    let markers = [
        "id=\"__next\"",
        "id='__next'",
        "id=\"root\"",
        "id='root'",
        "data-reactroot",
        "window.__initial_state__",
        "window.__nuxt",
        "ng-version",
        "data-hydrate",
    ];
    let marker_hit = markers.iter().any(|m| lower.contains(m));
    let script_count = re(r"(?is)<script\b").find_iter(raw_html).count();
    marker_hit || script_count >= 4
}

fn select_main_content(html: &str, min_chars: usize) -> Option<String> {
    let selectors = [
        r"(?is)<main[^>]*>(.*?)</main>",
        r"(?is)<article[^>]*>(.*?)</article>",
        r#"(?is)<[^>]+role=["']main["'][^>]*>(.*?)</[^>]+>"#,
        r#"(?is)<[^>]+(?:id|class)=["'][^"']*(?:content|docs-content|markdown-body|documentation)[^"']*["'][^>]*>(.*?)</[^>]+>"#,
    ];
    for pattern in selectors {
        if let Some(caps) = re(pattern).captures(html)
            && let Some(m) = caps.get(1)
        {
            let candidate = m.as_str().trim();
            if visible_text_len(candidate) >= min_chars {
                return Some(candidate.to_string());
            }
        }
    }
    None
}

fn pick_largest_block(html: &str) -> Option<String> {
    let block_re =
        re(r"(?is)<(?:section|div|article|main)[^>]*>(.*?)</(?:section|div|article|main)>");
    let mut best = None::<(usize, String)>;
    for caps in block_re.captures_iter(html) {
        let Some(m) = caps.get(1) else {
            continue;
        };
        let body = m.as_str();
        let score = visible_text_len(body);
        if score == 0 {
            continue;
        }
        match &best {
            Some((best_score, _)) if *best_score >= score => {}
            _ => best = Some((score, body.to_string())),
        }
    }
    best.map(|(_, body)| body)
}

fn html_to_markdown(html: &str, options: &ExtractOptions) -> String {
    let mut s = html.to_string();
    s = re(r"(?is)<(?:nav|footer|aside|form|button)[^>]*>.*?</(?:nav|footer|aside|form|button)>")
        .replace_all(&s, " ")
        .to_string();

    if options.include_tables {
        s = convert_tables(&s, options.max_table_rows, options.max_table_cols);
    } else {
        s = re(r"(?is)<table[^>]*>.*?</table>")
            .replace_all(&s, " ")
            .to_string();
    }

    if options.preserve_code_blocks {
        s = convert_code_blocks(&s);
    }

    s = convert_headings(&s);

    if options.include_links {
        s = convert_links(&s, options.strip_tracking_params);
    } else {
        s = re(r#"(?is)<a[^>]*>(.*?)</a>"#)
            .replace_all(&s, |caps: &Captures| {
                html_unescape(caps.get(1).map(|m| m.as_str()).unwrap_or(""))
            })
            .to_string();
    }

    s = re(r"(?is)<code[^>]*>(.*?)</code>")
        .replace_all(&s, |caps: &Captures| {
            format!(
                "`{}`",
                html_unescape(caps.get(1).map(|m| m.as_str()).unwrap_or(""))
            )
        })
        .to_string();
    s = re(r"(?is)<li[^>]*>(.*?)</li>")
        .replace_all(&s, |caps: &Captures| {
            format!(
                "\n- {}\n",
                html_unescape(caps.get(1).map(|m| m.as_str()).unwrap_or(""))
            )
        })
        .to_string();
    s = re(r"(?is)<(p|div|section|article|br)\b[^>]*>")
        .replace_all(&s, "\n")
        .to_string();
    s = re(r"(?is)</(p|div|section|article|br)\b[^>]*>")
        .replace_all(&s, "\n")
        .to_string();
    s = re(r"(?is)<[^>]+>").replace_all(&s, " ").to_string();
    html_unescape(&s)
}

fn convert_code_blocks(input: &str) -> String {
    re(r#"(?is)<pre>\s*<code([^>]*)>(.*?)</code>\s*</pre>"#)
        .replace_all(input, |caps: &Captures| {
            let attrs = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let lang = detect_lang_hint(attrs);
            let body = html_unescape(caps.get(2).map(|m| m.as_str()).unwrap_or(""))
                .replace("\r\n", "\n")
                .trim_end_matches('\n')
                .to_string();
            if lang.is_empty() {
                format!("\n```\n{body}\n```\n")
            } else {
                format!("\n```{lang}\n{body}\n```\n")
            }
        })
        .to_string()
}

fn detect_lang_hint(attrs: &str) -> String {
    let attr = attrs.to_ascii_lowercase();
    let patterns = [
        ("language-bash", "bash"),
        ("language-sh", "bash"),
        ("language-zsh", "zsh"),
        ("language-python", "python"),
        ("language-rust", "rust"),
        ("language-js", "javascript"),
        ("language-ts", "typescript"),
    ];
    for (needle, lang) in patterns {
        if attr.contains(needle) {
            return lang.to_string();
        }
    }
    String::new()
}

fn convert_headings(input: &str) -> String {
    let mut out = input.to_string();
    for level in 1..=6 {
        let hashes = "#".repeat(level);
        let pattern = format!(r"(?is)<h{level}[^>]*>(.*?)</h{level}>");
        out = re(&pattern)
            .replace_all(&out, |caps: &Captures| {
                format!(
                    "\n{hashes} {}\n",
                    html_unescape(caps.get(1).map(|m| m.as_str()).unwrap_or("")).trim()
                )
            })
            .to_string();
    }
    out
}

fn convert_links(input: &str, strip_tracking_params: bool) -> String {
    re(r#"(?is)<a[^>]*href=["']([^"']+)["'][^>]*>(.*?)</a>"#)
        .replace_all(input, |caps: &Captures| {
            let href = caps.get(1).map(|m| m.as_str()).unwrap_or_default();
            let text = html_unescape(caps.get(2).map(|m| m.as_str()).unwrap_or(""))
                .trim()
                .to_string();
            if text.is_empty() {
                return String::new();
            }
            let linked = if strip_tracking_params {
                sanitize_tracking_url(href)
            } else {
                href.to_string()
            };
            format!("[{text}]({linked})")
        })
        .to_string()
}

fn convert_tables(input: &str, max_rows: usize, max_cols: usize) -> String {
    let table_re = re(r"(?is)<table[^>]*>(.*?)</table>");
    let row_re = re(r"(?is)<tr[^>]*>(.*?)</tr>");
    let cell_re = re(r"(?is)<t[hd][^>]*>(.*?)</t[hd]>");

    table_re
        .replace_all(input, |caps: &Captures| {
            let Some(table) = caps.get(1).map(|m| m.as_str()) else {
                return String::new();
            };
            let mut rows = Vec::<Vec<String>>::new();
            for row_caps in row_re.captures_iter(table).take(max_rows.max(1)) {
                let Some(row_html) = row_caps.get(1).map(|m| m.as_str()) else {
                    continue;
                };
                let cells = cell_re
                    .captures_iter(row_html)
                    .take(max_cols.max(1))
                    .map(|c| {
                        html_unescape(c.get(1).map(|m| m.as_str()).unwrap_or_default())
                            .split_whitespace()
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
                    .collect::<Vec<_>>();
                if !cells.is_empty() {
                    rows.push(cells);
                }
            }
            if rows.is_empty() {
                return String::new();
            }

            let header = rows[0].clone();
            let cols = header.len();
            let mut md = String::new();
            md.push('|');
            md.push_str(&header.join("|"));
            md.push_str("|\n|");
            md.push_str(&vec!["---"; cols].join("|"));
            md.push_str("|\n");
            for row in rows.into_iter().skip(1) {
                let mut normalized = row;
                if normalized.len() < cols {
                    normalized.extend(vec![String::new(); cols - normalized.len()]);
                }
                normalized.truncate(cols);
                md.push('|');
                md.push_str(&normalized.join("|"));
                md.push_str("|\n");
            }
            format!("\n{md}\n")
        })
        .to_string()
}

fn normalize_markdown(markdown: &str) -> String {
    let mut out = Vec::<String>::new();
    let mut seen = std::collections::BTreeSet::<String>::new();
    let mut blank = false;
    let mut in_fence = false;

    for raw in markdown.lines() {
        let line = raw.trim_end().to_string();
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            out.push(line);
            blank = false;
            continue;
        }
        if in_fence {
            out.push(raw.to_string());
            blank = false;
            continue;
        }

        let compact = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if compact.is_empty() {
            if !blank {
                out.push(String::new());
                blank = true;
            }
            continue;
        }
        if is_navigation_noise(&compact) {
            continue;
        }
        if seen.insert(compact.to_ascii_lowercase()) {
            out.push(compact);
            blank = false;
        }
    }

    out.join("\n").trim().to_string()
}

fn normalize_spacing(markdown: &str) -> String {
    let mut out = Vec::<String>::new();
    let mut blank = false;
    for raw in markdown.lines() {
        let line = raw.trim_end();
        if line.trim().is_empty() {
            if !blank {
                out.push(String::new());
                blank = true;
            }
            continue;
        }
        out.push(line.to_string());
        blank = false;
    }
    out.join("\n").trim().to_string()
}

fn is_navigation_noise(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.starts_with("privacy policy")
        || lower.starts_with("cookie")
        || lower.starts_with("terms of service")
        || lower.contains("share on ")
        || lower.contains("follow us")
}

fn sanitize_tracking_url(input: &str) -> String {
    let trimmed = input.trim();
    let Ok(mut url) = Url::parse(trimmed) else {
        return trimmed.to_string();
    };
    let deny = ["fbclid", "gclid", "mc_cid", "mc_eid"];
    let kept = url
        .query_pairs()
        .filter(|(k, _)| {
            let key = k.as_ref();
            !key.starts_with("utm_") && !deny.contains(&key)
        })
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect::<Vec<_>>();
    url.set_query(None);
    if !kept.is_empty() {
        let mut qp = url.query_pairs_mut();
        for (k, v) in kept {
            qp.append_pair(&k, &v);
        }
    }
    url.to_string()
}

fn visible_text_len(html: &str) -> usize {
    let stripped = re(r"(?is)<[^>]+>").replace_all(html, " ");
    stripped.split_whitespace().count()
}

fn html_unescape(input: &str) -> String {
    input
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid regex")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_images_and_preserves_code() {
        let html = r#"<html><head><title>t</title></head><body><img src='x'/><pre><code class=\"language-sh\">echo hi</code></pre></body></html>"#;
        let (_t, md) = extract_markdown(html);
        assert!(!md.contains("!["));
        assert!(md.contains("```bash"));
        assert!(md.contains("echo hi"));
    }

    #[test]
    fn strips_tracking_query_params() {
        let clean = sanitize_tracking_url(
            "https://example.com/docs?a=1&utm_source=abc&fbclid=123&b=2&gclid=3",
        );
        assert!(clean.contains("a=1"));
        assert!(clean.contains("b=2"));
        assert!(!clean.contains("utm_source"));
        assert!(!clean.contains("fbclid"));
        assert!(!clean.contains("gclid"));
    }

    #[test]
    fn prefers_main_content_when_available() {
        let html = r#"
            <html><body>
            <div>nav nav nav</div>
            <main><h1>Docs</h1><p>Important content paragraph one.</p><p>Paragraph two.</p></main>
            </body></html>
        "#;
        let (_t, md) = extract_markdown(html);
        assert!(md.contains("Docs"));
        assert!(md.contains("Important content"));
    }

    #[test]
    fn can_disable_link_rendering() {
        let html = "<p><a href='https://example.com?a=1&utm_source=bad'>Example</a></p>";
        let opts = ExtractOptions {
            include_links: false,
            ..ExtractOptions::default()
        };
        let out = extract_markdown_with_options(html, &opts);
        assert!(out.markdown.contains("Example"));
        assert!(!out.markdown.contains("http"));
    }

    #[test]
    fn reports_dynamic_content_unavailable_for_shell_pages() {
        let html = r#"<html><body><div id=\"__next\"></div><script>window.__INITIAL_STATE__={};</script><script src='a.js'></script><script src='b.js'></script><script src='c.js'></script></body></html>"#;
        let opts = ExtractOptions {
            min_extracted_chars: 300,
            ..ExtractOptions::default()
        };
        let out = extract_markdown_with_options(html, &opts);
        assert_eq!(out.status, "dynamic_content_unavailable");
    }
}
