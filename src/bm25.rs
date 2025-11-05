// src/bm25.rs
// BM25 implementation for Flapjack search engine
// Stores: doc lengths, term frequencies in posting lists, global stats

use anyhow::{Context, Result};
use heed::types::*;
use heed::{Database, Env, EnvOpenOptions};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

// BM25 parameters - standard values from literature
pub const K1: f32 = 1.2;
pub const B: f32 = 0.75;

/// Document metadata required for BM25 scoring
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocMetadata {
    pub doc_id: u32,
    pub doc_length: u32, // Number of tokens in document
}

/// Posting list entry: doc_id + term frequency
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostingEntry {
    pub doc_id: u32,
    pub term_freq: u16, // How many times term appears in doc
}

/// Posting list for a term (delta-encoded doc_ids + term freqs)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostingList {
    pub entries: Vec<PostingEntry>,
    pub doc_freq: u32, // Number of documents containing this term
}

/// Global corpus statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorpusStats {
    pub total_docs: u32,
    pub total_tokens: u64,
    pub avg_doc_length: f32,
}

impl Default for CorpusStats {
    fn default() -> Self {
        Self {
            total_docs: 0,
            total_tokens: 0,
            avg_doc_length: 0.0,
        }
    }
}

/// BM25 index stored in LMDB
pub struct BM25Index {
    env: Env,
    // DB: doc_id (u32) -> DocMetadata
    doc_metadata_db: Database<U32<byteorder::NativeEndian>, SerdeBincode<DocMetadata>>,
    // DB: term (str) -> PostingList
    posting_lists_db: Database<Str, SerdeBincode<PostingList>>,
    // DB: single key "stats" -> CorpusStats
    corpus_stats_db: Database<Str, SerdeBincode<CorpusStats>>,
}

impl BM25Index {
    /// Create or open BM25 index at given path
    pub fn open<P: AsRef<Path>>(path: P, map_size: usize) -> Result<Self> {
        std::fs::create_dir_all(&path)?;
        
        let env = unsafe {
            EnvOpenOptions::new()
                .map_size(map_size)
                .max_dbs(10)
                .open(path)?
        };

        let mut wtxn = env.write_txn()?;
        
        let doc_metadata_db = env.create_database(&mut wtxn, Some("doc_metadata"))?;
        let posting_lists_db = env.create_database(&mut wtxn, Some("posting_lists"))?;
        let corpus_stats_db = env.create_database(&mut wtxn, Some("corpus_stats"))?;
        
        wtxn.commit()?;

        Ok(Self {
            env,
            doc_metadata_db,
            posting_lists_db,
            corpus_stats_db,
        })
    }

    /// Index a batch of documents
    /// docs: vec of (doc_id, tokens)
    pub fn index_documents(&self, docs: Vec<(u32, Vec<String>)>) -> Result<()> {
        let mut wtxn = self.env.write_txn()?;

        // Load or create corpus stats
        let mut stats = self
            .corpus_stats_db
            .get(&wtxn, "stats")?
            .unwrap_or_default();

        // Build term -> list of (doc_id, term_freq)
        let mut term_postings: HashMap<String, Vec<PostingEntry>> = HashMap::new();

        for (doc_id, tokens) in &docs {
            let doc_length = tokens.len() as u32;

            // Store doc metadata
            let metadata = DocMetadata {
                doc_id: *doc_id,
                doc_length,
            };
            self.doc_metadata_db.put(&mut wtxn, doc_id, &metadata)?;

            // Update corpus stats
            stats.total_docs += 1;
            stats.total_tokens += doc_length as u64;

            // Count term frequencies in this doc
            let mut term_freqs: HashMap<&str, u16> = HashMap::new();
            for token in tokens {
                *term_freqs.entry(token.as_str()).or_insert(0) += 1;
            }

            // Add to posting lists
            for (term, freq) in term_freqs {
                term_postings
                    .entry(term.to_string())
                    .or_default()
                    .push(PostingEntry {
                        doc_id: *doc_id,
                        term_freq: freq,
                    });
            }
        }

        // Update avg doc length
        stats.avg_doc_length = stats.total_tokens as f32 / stats.total_docs as f32;
        self.corpus_stats_db.put(&mut wtxn, "stats", &stats)?;

        // Store posting lists
        for (term, mut entries) in term_postings {
            // Sort by doc_id for delta encoding later
            entries.sort_by_key(|e| e.doc_id);

            // Load existing posting list if any
            let mut posting_list = self
                .posting_lists_db
                .get(&wtxn, term.as_str())?
                .unwrap_or_else(|| PostingList {
                    entries: Vec::new(),
                    doc_freq: 0,
                });

            // Merge new entries
            posting_list.entries.extend(entries);
            posting_list.doc_freq = posting_list.entries.len() as u32;

            self.posting_lists_db.put(&mut wtxn, term.as_str(), &posting_list)?;
        }

        wtxn.commit()?;
        Ok(())
    }

    /// Calculate BM25 score for a document given query terms
    pub fn score_document(&self, doc_id: u32, query_terms: &[String]) -> Result<f32> {
        let rtxn = self.env.read_txn()?;

        // Get doc metadata
        let doc_meta = self
            .doc_metadata_db
            .get(&rtxn, &doc_id)?
            .context("Document not found")?;

        // Get corpus stats
        let stats = self
            .corpus_stats_db
            .get(&rtxn, "stats")?
            .unwrap_or_default();

        let mut score = 0.0_f32;

        for term in query_terms {
            // Get posting list for term
            let posting_list = match self.posting_lists_db.get(&rtxn, term.as_str())? {
                Some(pl) => pl,
                None => continue, // Term not in corpus
            };

            // Find term frequency in this doc
            let term_freq = posting_list
                .entries
                .iter()
                .find(|e| e.doc_id == doc_id)
                .map(|e| e.term_freq as f32)
                .unwrap_or(0.0);

            if term_freq == 0.0 {
                continue;
            }

            // Calculate IDF
            let df = posting_list.doc_freq as f32;
            let idf = ((stats.total_docs as f32 - df + 0.5) / (df + 0.5) + 1.0).ln();

            // Calculate term frequency component with saturation
            let doc_len_norm = 1.0 - B + B * (doc_meta.doc_length as f32 / stats.avg_doc_length);
            let tf_component = (term_freq * (K1 + 1.0)) / (term_freq + K1 * doc_len_norm);

            score += idf * tf_component;
        }

        Ok(score)
    }

    /// Search: return top-k doc_ids by BM25 score
    pub fn search(&self, query_terms: &[String], top_k: usize) -> Result<Vec<(u32, f32)>> {
        let rtxn = self.env.read_txn()?;

        // Get all candidate doc_ids (union of posting lists)
        let mut candidate_docs = std::collections::HashSet::new();

        for term in query_terms {
            if let Some(posting_list) = self.posting_lists_db.get(&rtxn, term.as_str())? {
                for entry in &posting_list.entries {
                    candidate_docs.insert(entry.doc_id);
                }
            }
        }

        // Score all candidates
        let mut scored_docs: Vec<(u32, f32)> = candidate_docs
            .into_iter()
            .map(|doc_id| {
                let score = self.score_document(doc_id, query_terms).unwrap_or(0.0);
                (doc_id, score)
            })
            .collect();

        // Sort by score descending
        scored_docs.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored_docs.truncate(top_k);

        Ok(scored_docs)
    }

    /// Get corpus stats for inspection
    pub fn get_stats(&self) -> Result<CorpusStats> {
        let rtxn = self.env.read_txn()?;
        Ok(self
            .corpus_stats_db
            .get(&rtxn, "stats")?
            .unwrap_or_default())
    }

    /// Count posting lists in index
    pub fn count_terms(&self) -> Result<usize> {
        let rtxn = self.env.read_txn()?;
        Ok(self.posting_lists_db.len(&rtxn)? as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn simple_tokenize(text: &str) -> Vec<String> {
        text.to_lowercase()
            .split_whitespace()
            .map(|s| s.to_string())
            .collect()
    }

    #[test]
    fn test_bm25_basic() {
        let temp_dir = tempfile::tempdir().unwrap();
        let index = BM25Index::open(temp_dir.path(), 100 * 1024 * 1024).unwrap();

        // Index some documents
        let docs = vec![
            (1, simple_tokenize("the quick brown fox jumps over the lazy dog")),
            (2, simple_tokenize("the dog sat on the mat")),
            (3, simple_tokenize("the cat sat on the mat")),
        ];

        index.index_documents(docs).unwrap();

        // Search
        let query = simple_tokenize("cat dog");
        let results = index.search(&query, 10).unwrap();

        println!("Results: {:?}", results);
        assert!(!results.is_empty());
        
        // Doc 3 should score highest (has "cat")
        // Doc 2 should score second (has "dog")
        assert_eq!(results[0].0, 3);
        assert_eq!(results[1].0, 2);
    }

    #[test]
    fn test_corpus_stats() {
        let temp_dir = tempfile::tempdir().unwrap();
        let index = BM25Index::open(temp_dir.path(), 100 * 1024 * 1024).unwrap();

        let docs = vec![
            (1, simple_tokenize("hello world")),
            (2, simple_tokenize("hello rust programming")),
        ];

        index.index_documents(docs).unwrap();

        let stats = index.get_stats().unwrap();
        assert_eq!(stats.total_docs, 2);
        assert_eq!(stats.total_tokens, 5); // 2 + 3
        assert_eq!(stats.avg_doc_length, 2.5);
    }
}
