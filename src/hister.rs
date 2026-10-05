//! The parts of hister's HTTP API that semsearch uses.

use crate::Source;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// hister lets command-line clients past its CSRF check with this Origin.
const ORIGIN: &str = "hister://";
/// hister rejects a batch with more operations than this.
pub const MAX_BATCH: usize = 100;

#[derive(Serialize, Clone)]
pub struct Doc {
    pub url: String,
    pub title: String,
    pub text: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub label: String,
    /// Unix seconds: first visit, or first commit. Zero lets hister use the
    /// time of indexing. hister keeps the stored value when a document is
    /// added again, so changing it takes a delete first.
    #[serde(skip_serializing_if = "is_zero")]
    pub added: i64,
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
    pub similarity: f64,
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
}

#[derive(Deserialize)]
struct SemanticHit {
    doc_id: String,
    similarity: f64,
    matched_chunk: String,
    document: Option<Value>,
}

/// Semantic hits from each source, merged by similarity. Scores compare
/// across sources because every instance embeds with the same model. Each
/// instance returns up to its `semantic_search.result_limit` hits.
pub fn search(sources: &[Source], query: &str) -> Result<Vec<Hit>> {
    let q = json!({ "text": query, "semantic_enabled": true }).to_string();
    let mut hits = Vec::new();
    for &source in sources {
        let server = source.server();
        let mut resp = agent()
            .get(format!("{server}/search"))
            .query("query", &q)
            .header("Origin", ORIGIN)
            .call()
            .with_context(|| format!("GET {server}/search"))?;
        let body: SearchResponse = resp.body_mut().read_json().context("search response")?;
        let keyword = body.documents.unwrap_or_default();
        for h in body.semantic_hits.unwrap_or_default() {
            let url = h.document.as_ref().and_then(|d| d["url"].as_str()).unwrap_or(&h.doc_id).to_string();
            let doc = h.document.as_ref().or_else(|| keyword.iter().find(|d| d["url"].as_str() == Some(&url)));
            let field = |k: &str| doc.and_then(|d| d[k].as_i64()).unwrap_or(0);
            hits.push(Hit {
                source: source.name(),
                similarity: h.similarity,
                title: doc.and_then(|d| d["title"].as_str()).unwrap_or("").to_string(),
                added: field("added"),
                // hister sets `updated` to the time of the add for anything but
                // local files, so semsearch keeps the real time in metadata.
                // A page the extension captured has none, and there hister's
                // `updated` is the last submission, which is the last visit.
                updated: doc.and_then(|d| d["metadata"]["updated"].as_i64()).unwrap_or_else(|| field("updated")),
                visits: doc.and_then(|d| d["metadata"]["visits"].as_u64()),
                url,
                chunk: h.matched_chunk,
            });
        }
    }
    hits.sort_by(|a, b| b.similarity.total_cmp(&a.similarity));
    Ok(hits)
}
