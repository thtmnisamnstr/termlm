use chrono::Utc;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use std::hint::black_box;
use termlm_indexer::{Chunk, HybridRetriever, RetrievalQuery};

fn build_chunks(count: usize) -> Vec<Chunk> {
    let sections = ["NAME", "SYNOPSIS", "OPTIONS", "EXAMPLES"];
    let mut chunks = Vec::with_capacity(count);
    let now = Utc::now();
    for i in 0..count {
        let command = format!("cmd{}", i % 3000);
        let section = sections[i % sections.len()].to_string();
        let text = format!(
            "{command} supports --flag{} and pattern search over workspace file {}",
            i % 7,
            i % 101
        );
        chunks.push(Chunk {
            command_name: command.clone(),
            path: format!("/usr/local/share/man/man1/{command}.1"),
            extraction_method: "man".to_string(),
            section_name: section,
            chunk_index: i % 4,
            total_chunks: 4,
            doc_hash: format!("h{i:08x}"),
            extracted_at: now,
            text,
        });
    }
    chunks
}

fn bench_hybrid_search(c: &mut Criterion) {
    let mut group = c.benchmark_group("hybrid_retrieval");
    group.sample_size(20);

    for &size in &[5_000usize, 20_000usize] {
        let retriever = HybridRetriever::with_dim(build_chunks(size), 384);
        group.throughput(Throughput::Elements(size as u64));

        let hit_query = RetrievalQuery::new("cmd42 --flag2 options", 8, 0.0);
        group.bench_with_input(BenchmarkId::new("search_hit", size), &hit_query, |b, q| {
            b.iter(|| {
                black_box(retriever.search(black_box(q)));
            })
        });

        let miss_query = RetrievalQuery::new("nonexistent command from docs", 8, 0.0);
        group.bench_with_input(
            BenchmarkId::new("search_lexical_miss", size),
            &miss_query,
            |b, q| {
                b.iter(|| {
                    black_box(retriever.search(black_box(q)));
                })
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_hybrid_search);
criterion_main!(benches);
