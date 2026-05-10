use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Default, Clone)]
pub struct LexicalIndex {
    postings: BTreeMap<String, BTreeSet<usize>>,
    doc_ids: BTreeSet<usize>,
}

impl LexicalIndex {
    pub fn from_postings(postings: BTreeMap<String, BTreeSet<usize>>) -> Self {
        let mut doc_ids = BTreeSet::new();
        for ids in postings.values() {
            doc_ids.extend(ids.iter().copied());
        }
        Self { postings, doc_ids }
    }

    pub fn insert(&mut self, doc_id: usize, text: &str) {
        self.doc_ids.insert(doc_id);
        for tok in tokenize(text) {
            self.postings.entry(tok).or_default().insert(doc_id);
        }
    }

    pub fn search(&self, query: &str) -> Vec<(usize, f32)> {
        let mut query_tf = BTreeMap::<String, u32>::new();
        for tok in tokenize(query) {
            *query_tf.entry(tok).or_insert(0) += 1;
        }
        if query_tf.is_empty() {
            return Vec::new();
        }
        let query_unique_terms = query_tf.len().max(1) as f32;
        let doc_count = self.doc_ids.len().max(1) as f32;
        let mut scores: BTreeMap<usize, f32> = BTreeMap::new();
        let mut matched_terms = BTreeMap::<usize, usize>::new();

        for (tok, qtf) in query_tf {
            if let Some(ids) = self.postings.get(&tok) {
                let df = ids.len().max(1) as f32;
                let idf = (((doc_count - df + 0.5) / (df + 0.5)) + 1.0).ln().max(0.0);
                let query_weight = 1.0 + (qtf as f32).ln_1p();
                for id in ids {
                    *scores.entry(*id).or_insert(0.0) += idf * query_weight;
                    *matched_terms.entry(*id).or_insert(0) += 1;
                }
            }
        }
        if scores.is_empty() {
            return Vec::new();
        }

        for (doc_id, score) in &mut scores {
            let covered = matched_terms.get(doc_id).copied().unwrap_or(0) as f32;
            let coverage = (covered / query_unique_terms).clamp(0.0, 1.0);
            *score += coverage * 0.2;
        }

        let mut out: Vec<(usize, f32)> = scores.into_iter().collect();
        out.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        out
    }
}

fn tokenize(input: &str) -> Vec<String> {
    input
        .split(|c: char| !c.is_alphanumeric() && c != '-' && c != '_')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn favors_docs_with_rare_terms() {
        let mut idx = LexicalIndex::default();
        idx.insert(0, "git status");
        idx.insert(1, "git commit amend");
        idx.insert(2, "git branch");

        let hits = idx.search("git commit");
        assert!(!hits.is_empty());
        assert_eq!(hits[0].0, 1);
    }

    #[test]
    fn repeated_query_terms_increase_score() {
        let mut idx = LexicalIndex::default();
        idx.insert(0, "grep pattern");
        let once = idx.search("grep");
        let repeated = idx.search("grep grep grep");
        assert!(!once.is_empty());
        assert!(!repeated.is_empty());
        assert!(repeated[0].1 > once[0].1);
    }
}
