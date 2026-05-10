use criterion::{Criterion, criterion_group, criterion_main};
use std::hint::black_box;
use termlm_web::extract::extract_markdown;

fn sample_html() -> String {
    let mut html = String::from("<html><head><title>Docs</title></head><body><main>");
    for i in 0..600 {
        html.push_str(&format!("<h2>Section {i}</h2><p>Paragraph {i} with <a href='https://example.com/page?utm_source=test&id={i}'>link</a> and <code>cmd --flag {i}</code>.</p>"));
    }
    html.push_str("<table><tr><th>Flag</th><th>Meaning</th></tr>");
    for i in 0..25 {
        html.push_str(&format!("<tr><td>--opt-{i}</td><td>option {i}</td></tr>"));
    }
    html.push_str("</table><pre><code class='language-bash'>echo hello\nls -la</code></pre>");
    html.push_str("</main></body></html>");
    html
}

fn bench_extract(c: &mut Criterion) {
    let html = sample_html();
    c.bench_function("web_extract_markdown", |b| {
        b.iter(|| {
            let (_, md) = extract_markdown(black_box(&html));
            black_box(md);
        })
    });
}

criterion_group!(benches, bench_extract);
criterion_main!(benches);
