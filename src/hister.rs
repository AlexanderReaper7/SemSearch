//! The parts of hister's HTTP API that semsearch uses.

use crate::Source;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// hister lets command-line clients past its CSRF check with this Origin.
const ORIGIN: &str = "hister://";
/// hister rejects a batch with more operations than this.
pub const MAX_BATCH: usize = 100;

/// hister's document types, as the number its API takes.
#[derive(Clone, Copy)]
pub enum Kind {
    Web = 0,
    Code = 3,
}

impl Serialize for Kind {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u8(*self as u8)
    }
}

#[derive(Serialize, Clone)]
pub struct Doc {
    pub url: String,
    pub title: String,
    pub text: String,
    #[serde(rename = "type")]
    pub kind: Kind,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub label: String,
    /// Unix seconds: first visit, or first commit. Zero lets hister use the
    /// time of indexing. The fork keeps the earliest value any add sent.
    #[serde(skip_serializing_if = "is_zero")]
    pub added: i64,
    /// Unix seconds: last visit, or last commit or modification.
    #[serde(skip_serializing_if = "is_zero")]
    pub updated: i64,
    #[serde(skip_serializing_if = "Value::is_null")]
    pub metadata: Value,
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        // Embedding runs inside the add request, and a batch on the CPU
        // embedder takes minutes.
        .timeout_global(Some(std::time::Duration::from_secs(1800)))
        .http_status_as_error(false)
        .build()
        .into()
}

fn batch(server: &str, ops: Vec<Value>) -> Result<Vec<Value>> {
    let mut resp = agent()
        .post(format!("{server}/api/batch"))
        .header("Origin", ORIGIN)
        .send_json(json!({ "ops": ops }))
        .with_context(|| format!("POST {server}/api/batch"))?;
    let status = resp.status();
    let body: Value = resp.body_mut().read_json().context("batch response")?;
    if !status.is_success() {
        bail!("batch returned {status}: {body}");
    }
    Ok(body["results"].as_array().cloned().unwrap_or_default())
}

pub fn add(server: &str, docs: &[Doc]) -> Result<usize> {
    let mut failed = 0;
    for part in docs.chunks(MAX_BATCH) {
        let ops = part
            .iter()
            .map(|d| {
                let mut v = serde_json::to_value(d).expect("Doc serializes");
                v["op"] = json!("add");
                v
            })
            .collect();
        for (doc, r) in part.iter().zip(batch(server, ops)?) {
            let status = r["status"].as_u64().unwrap_or(0);
            if !(200..300).contains(&status) {
                failed += 1;
                eprintln!("add {} -> {status} {}", doc.url, r["error"]);
            }
        }
    }
    Ok(failed)
}

pub fn delete(server: &str, urls: &[String]) -> Result<()> {
    for part in urls.chunks(MAX_BATCH) {
        let ops = part.iter().map(|u| json!({ "op": "delete", "url": u })).collect();
        batch(server, ops)?;
    }
    Ok(())
}

/// The stored document for each of `urls`, if the instance holds it.
pub fn get(server: &str, urls: &[String]) -> Result<Vec<Option<Value>>> {
    let mut out = Vec::with_capacity(urls.len());
    for part in urls.chunks(MAX_BATCH) {
        let ops = part.iter().map(|u| json!({ "op": "get", "url": u })).collect();
        out.extend(
            batch(server, ops)?
                .into_iter()
                .map(|mut r| (r["status"].as_u64() == Some(200)).then(|| r["document"].take())),
        );
    }
    Ok(out)
}

#[derive(Serialize, Debug)]
pub struct Hit {
    pub source: &'static str,
    /// Cosine similarity, or None for a keyword hit shown because the
    /// semantic search failed.
    pub similarity: Option<f64>,
    pub url: String,
    pub title: String,
    pub chunk: String,
    /// Unix seconds, as stored: first visit or commit, and last visit or change.
    pub added: i64,
    pub updated: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visits: Option<u64>,
}

#[derive(Deserialize)]
struct SearchResponse {
    semantic_hits: Option<Vec<SemanticHit>>,
    /// Keyword hits. A semantic hit that is also here has no `document` of
    /// its own, so its title comes from this list.
    documents: Option<Vec<Value>>,
    /// Set by the fork when the query embedding or the vector search failed.
    semantic_error: Option<String>,
}

#[derive(Deserialize)]
struct SemanticHit {
    doc_id: String,
    similarity: f64,
    matched_chunk: String,
    document: Option<Value>,
}

/// A search slower than this is reported, since the embedder shares the CPU
/// with whatever else runs. See docs/decisions.md.
const SLOW: std::time::Duration = std::time::Duration::from_secs(3);

pub struct Search<'a> {
    pub query: &'a str,
    pub since: Option<i64>,
    pub before: Option<i64>,
}

/// Semantic hits, from one source or, with `source` None, from all of them.
/// All sources live in one instance, so a source is a type filter. If the
/// semantic search failed, the keyword hits are returned instead, without a
/// similarity, so a degraded search still shows something.
pub fn search(source: Option<Source>, s: &Search) -> Result<Vec<Hit>> {
    let server = server();
    let text = match source {
        Some(src) => format!("{} {}", src.filter(), s.query),
        None => s.query.to_string(),
    };
    let mut q = json!({ "text": text, "semantic_enabled": true });
    if let Some(t) = s.since {
        q["date_from"] = json!(t);
    }
    if let Some(t) = s.before {
        // hister's range is inclusive; --before is not.
        q["date_to"] = json!(t - 1);
    }
    let started = std::time::Instant::now();
    let mut resp = agent()
        .get(format!("{server}/search"))
        .query("query", q.to_string())
        .header("Origin", ORIGIN)
        .call()
        .with_context(|| format!("GET {server}/search"))?;
    let body: SearchResponse = resp.body_mut().read_json().context("search response")?;
    let took = started.elapsed();
    if took > SLOW {
        eprintln!("semsearch: search took {:.1} s", took.as_secs_f64());
    }
    let keyword = body.documents.unwrap_or_default();
    if let Some(err) = body.semantic_error {
        eprintln!("semsearch: semantic search failed, showing keyword hits: {err}");
        return Ok(keyword.iter().map(|d| hit(None, d, d["url"].as_str().unwrap_or_default(), String::new())).collect());
    }
    let mut hits: Vec<Hit> = body
        .semantic_hits
        .unwrap_or_default()
        .into_iter()
        .map(|h| {
            let url = h.document.as_ref().and_then(|d| d["url"].as_str()).unwrap_or(&h.doc_id).to_string();
            let doc = h.document.as_ref().or_else(|| keyword.iter().find(|d| d["url"].as_str() == Some(&url)));
            hit(Some(h.similarity), doc.unwrap_or(&Value::Null), &url, h.matched_chunk)
        })
        .collect();
    hits.sort_by(|a, b| b.similarity.unwrap_or(-1.0).total_cmp(&a.similarity.unwrap_or(-1.0)));
    Ok(hits)
}

fn hit(similarity: Option<f64>, doc: &Value, url: &str, chunk: String) -> Hit {
    Hit {
        source: Source::of_type(doc["type"].as_u64()).map_or("unknown", Source::name),
        similarity,
        title: doc["title"].as_str().unwrap_or("").to_string(),
        added: doc["added"].as_i64().unwrap_or(0),
        updated: doc["updated"].as_i64().unwrap_or(0),
        visits: doc["metadata"]["visits"].as_u64(),
        url: url.to_string(),
        chunk,
    }
}

/// The one hister instance that holds every source.
pub fn server() -> String {
    std::env::var("SEMSEARCH_URL").unwrap_or_else(|_| "http://127.0.0.1:4433".to_string())
}
