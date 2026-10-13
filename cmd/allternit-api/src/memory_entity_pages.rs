//! Entity pages: an entity layer for agentic search (C3, CorpusMap pattern).
//!
//! Mentions of the same entity are resolved across one user's memory
//! offline, and every recurring entity gets a page that aggregates what is
//! known about it and links to every fact, observation, document and note
//! that mentions it. Agents follow entities instead of re-searching
//! (`agency_api::memory_context::recall` puts the pages of the entities a goal
//! names ahead of the raw hits). No LLM calls, no embeddings.
//!
//! **Extraction** ([`extract`]), per target text:
//! - structured: URLs, emails, `@handles`, code identifiers (backticked
//!   tokens, `a::b` paths, snake_case);
//! - capitalized spans of up to [`MAX_SPAN_WORDS`] words (`Bank of America`,
//!   `ACME`), joined over `of`/`&`/`de`/…, never across punctuation; a
//!   sentence-initial function word is dropped (`The`, `Please`, …);
//! - the user's `memory_entities` names and aliases (`properties.aliases`),
//!   matched as word n-grams in any case (one-word lowercase matches excepted).
//!
//! **Normalization / merging:** a mention's key is its surface lowercased,
//! without a possessive or a leading `the`, alphanumerics only (`Acme Corp.`
//! = `ACME corp` = `acme-corp`); structured kinds are prefixed (`mailto:`,
//! `url:`, `@`, `code:`). Keys merge into one entity by exact key match and by
//! explicit alias evidence: `Allternit Platform (AP)` when the acronym is the
//! initials, `X aka Y` / `also known as`, and a seed's aliases (union-find).
//! Embedding similarity is deliberately not used for merging: names are short,
//! EmbeddingGemma scores sibling names (`Project Apollo` / `Project Artemis`)
//! close, the hash fallback would make merges machine-dependent, and a wrong
//! merge silently pollutes two pages. Exact evidence keeps pages deterministic.
//!
//! **Incremental:** V255 triggers queue a target whenever its text changes
//! (insert / update of the text / delete). [`run_pass`] (run by the memory
//! indexer, `memory_index::index_pending`) re-reads only queued targets,
//! replaces their mentions when they differ, and marks the touched keys dirty;
//! then for each dirty user it re-resolves keys (cheap: keys and links only)
//! and regenerates just the pages of affected entities, writing a page only
//! when its hash changes. A new or renamed seed re-queues the targets whose
//! keyword index (`memory_index_fts`) matches its names, so older texts pick
//! up lowercase mentions of it.
//!
//! **Pages:** an entity gets a page when it is a seed or is mentioned by at
//! least two targets. The page holds the canonical name (seed name, else the
//! most frequent surface), aliases, kind, the top facts / observations /
//! documents / notes about it (most mentions first, one-line snippet around
//! the first mention) and related entities by co-mention. Ids are
//! `ent_<sha256(user, canonical key)>`; when two entities merge the merged
//! page takes the canonical key's id and the other page is deleted.
//!
//! Routes (authenticated; every query is filtered by the caller's user id):
//! `GET /api/v1/memory/entities/:id` (page + mention links). Search is
//! `GET /api/v1/memory/entities?q=`, whose response gains `pages` next to the
//! legacy `entities` list (`memory_routes::list_entities` calls
//! [`search_pages`]).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use once_cell::sync::Lazy;
use regex::Regex;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::{auth::AuthUser, AppState};

pub const MAX_SPAN_WORDS: usize = 5;
const MAX_NGRAM: usize = 4;
const MAX_KEY_CHARS: usize = 80;
/// Targets a page needs when it is not a seed.
pub const MIN_TARGETS: usize = 2;
const PAGE_FACTS: usize = 5;
const PAGE_OBSERVATIONS: usize = 3;
const PAGE_DOCUMENTS: usize = 8;
const PAGE_NOTES: usize = 3;
const PAGE_RELATED: usize = 8;
const PAGE_ALIASES: usize = 10;
const ITEM_CHARS: usize = 240;
const SNIPPET_CHARS: usize = 160;
/// Targets re-queued per new seed name.
const SEED_REQUEUE_CAP: i64 = 1000;

// ─── Extraction ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mention {
    pub key: String,
    pub kind: &'static str,
    pub surface: String,
    pub pos: usize,
}

#[derive(Debug, Default, Clone)]
pub struct Extracted {
    pub mentions: Vec<Mention>,
    /// Alias evidence: pairs of keys of the same entity (ordered a < b).
    pub links: BTreeSet<(String, String)>,
}

fn pair(a: &str, b: &str) -> Option<(String, String)> {
    match a.cmp(b) {
        std::cmp::Ordering::Less => Some((a.into(), b.into())),
        std::cmp::Ordering::Greater => Some((b.into(), a.into())),
        std::cmp::Ordering::Equal => None,
    }
}

fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn clip(s: &str, n: usize) -> String {
    let s = collapse(s);
    if s.chars().count() <= n {
        s
    } else {
        format!("{}…", s.chars().take(n).collect::<String>().trim_end())
    }
}

fn strip_possessive(s: &str) -> &str {
    s.strip_suffix("'s").or_else(|| s.strip_suffix("’s")).unwrap_or(s)
}

/// Key of a name: lowercase, no possessive / leading `the`, alphanumerics.
pub fn name_key(surface: &str) -> Option<String> {
    let s = strip_possessive(surface.trim());
    let lower = s.to_lowercase();
    let lower = lower.strip_prefix("the ").unwrap_or(&lower);
    let k: String = lower.chars().filter(|c| c.is_alphanumeric()).collect();
    let n = k.chars().count();
    ((2..=MAX_KEY_CHARS).contains(&n) && !k.chars().all(|c| c.is_ascii_digit())).then_some(k)
}

static URL_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r#"https?://[^\s<>()\[\]"'`]+"#).unwrap());
static EMAIL_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\b[a-z0-9][a-z0-9._%+-]*@[a-z0-9-]+(?:\.[a-z0-9-]+)*\.[a-z]{2,}\b").unwrap());
static HANDLE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?:^|[\s(\[])(@[A-Za-z0-9_][A-Za-z0-9_-]{1,38})").unwrap());
static TICK_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"`([A-Za-z_][A-Za-z0-9_:./-]{1,80})`").unwrap());
static PATH_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\b[A-Za-z_][A-Za-z0-9_]*(?:::[A-Za-z_][A-Za-z0-9_]*)+").unwrap());
static SNAKE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\b[a-z][a-z0-9]*(?:_[a-z0-9]+)+\b").unwrap());

/// URLs, emails, handles and code identifiers, plus the byte ranges they cover.
fn structured(text: &str) -> (Vec<Mention>, Vec<(usize, usize)>) {
    let mut out = vec![];
    let mut covered: Vec<(usize, usize)> = vec![];
    let overlaps = |covered: &[(usize, usize)], s: usize, e: usize| covered.iter().any(|&(a, b)| s < b && a < e);
    let add = |out: &mut Vec<Mention>, covered: &mut Vec<(usize, usize)>, s: usize, e: usize, kind: &'static str, key: String| {
        if overlaps(covered, s, e) || key.len() > MAX_KEY_CHARS + 8 {
            return;
        }
        covered.push((s, e));
        out.push(Mention { key, kind, surface: text[s..e].to_string(), pos: s });
    };
    for m in URL_RE.find_iter(text) {
        let u = m.as_str().trim_end_matches(|c: char| ".,;:!?".contains(c));
        let rest = u.split_once("://").map(|x| x.1).unwrap_or(u).to_lowercase();
        let rest = rest.strip_prefix("www.").unwrap_or(&rest).trim_end_matches('/').to_string();
        if !rest.is_empty() {
            add(&mut out, &mut covered, m.start(), m.start() + u.len(), "url", format!("url:{rest}"));
        }
    }
    for m in EMAIL_RE.find_iter(text) {
        add(&mut out, &mut covered, m.start(), m.end(), "email", format!("mailto:{}", m.as_str().to_lowercase()));
    }
    for c in HANDLE_RE.captures_iter(text) {
        let m = c.get(1).unwrap();
        add(&mut out, &mut covered, m.start(), m.end(), "handle", m.as_str().to_lowercase());
    }
    for c in TICK_RE.captures_iter(text) {
        let m = c.get(1).unwrap();
        add(&mut out, &mut covered, m.start(), m.end(), "code", format!("code:{}", m.as_str().to_lowercase()));
    }
    for re in [&*PATH_RE, &*SNAKE_RE] {
        for m in re.find_iter(text) {
            add(&mut out, &mut covered, m.start(), m.end(), "code", format!("code:{}", m.as_str().to_lowercase()));
        }
    }
    (out, covered)
}

#[derive(Debug, Clone)]
struct Tok<'a> {
    word: &'a str,
    start: usize,
    end: usize,
    /// Leading punctuation (`(AP)`, quotes).
    opens: bool,
    /// Trailing clause punctuation.
    closes: bool,
    sent_start: bool,
}

const LEAD: &[char] = &['"', '\'', '(', '[', '{', '<', '*', '_', '“', '‘', '`'];
const TRAIL: &[char] = &['.', ',', ';', ':', '!', '?', ')', ']', '}', '>', '"', '\'', '*', '_', '”', '’', '`'];

fn tokens(text: &str) -> Vec<Tok<'_>> {
    let mut out = vec![];
    let mut sent = true;
    let mut start: Option<usize> = None;
    for (idx, c) in text.char_indices().chain(std::iter::once((text.len(), ' '))) {
        if !c.is_whitespace() {
            start.get_or_insert(idx);
            continue;
        }
        if let Some(s) = start.take() {
            let raw = &text[s..idx];
            let lead = raw.trim_start_matches(LEAD);
            let lead_len = raw.len() - lead.len();
            let word = lead.trim_end_matches(TRAIL);
            let trailing = &lead[word.len()..];
            let ws = s + lead_len;
            out.push(Tok {
                word,
                start: ws,
                end: ws + word.len(),
                opens: lead_len > 0,
                closes: trailing.contains(['.', ',', ';', ':', '!', '?', ')', ']', '}']),
                sent_start: sent,
            });
            sent = trailing.contains(['.', '!', '?', ':']) || (!word.is_empty() && word.chars().all(|c| "-*•#–—>|".contains(c)));
        }
        if c == '\n' {
            sent = true;
        }
    }
    out
}

const CONNECTORS: &[&str] = &["of", "for", "&", "de", "del", "der", "la", "le", "van", "von", "du", "da", "di"];
/// Capitalized words that are not names.
const STOP_CAPS: &[&str] = &[
    "monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday", "january", "february", "march",
    "april", "may", "june", "july", "august", "september", "october", "november", "december", "ok", "okay", "hi",
    "hello", "thanks", "thank", "yes", "no", "todo", "fyi", "asap", "am", "pm", "eta", "i'm", "i've", "i'll", "i'd",
    "tbd", "n/a", "etc",
];
/// Words dropped from the start of a sentence-initial span.
const COMMON_START: &[&str] = &[
    "the", "a", "an", "this", "that", "these", "those", "it", "its", "we", "our", "my", "your", "their", "his",
    "her", "he", "she", "they", "you", "in", "on", "at", "for", "if", "when", "after", "before", "please", "also",
    "but", "and", "so", "then", "today", "yesterday", "tomorrow", "next", "last", "every", "each", "all", "some",
    "new", "no", "not", "with", "from", "to", "by", "as", "of", "is", "are", "was", "were", "has", "have", "had",
    "do", "does", "did", "can", "could", "should", "would", "will", "here", "there", "what", "who", "why", "how",
    "where", "which", "hey", "dear", "let", "make", "send", "write", "ask", "tell", "call", "check", "update",
    "remind", "note", "email", "schedule", "book", "review", "draft", "read", "find", "get", "set", "add", "move",
    "use", "user", "assistant", "re", "fwd", "since", "while", "until", "because", "although", "however", "maybe",
];

fn is_cap(w: &str) -> bool {
    let mut cs = w.chars();
    let Some(f) = cs.next() else { return false };
    f.is_uppercase()
        && w.chars().count() >= 2
        && w.chars().all(|c| c.is_alphanumeric() || matches!(c, '-' | '\'' | '’' | '&'))
        && !STOP_CAPS.contains(&w.to_lowercase().as_str())
}

fn is_acronym(w: &str) -> bool {
    (2..=10).contains(&w.chars().count())
        && w.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        && w.chars().filter(|c| c.is_ascii_uppercase()).count() >= 2
}

/// Capitalized spans as inclusive token ranges.
fn cap_spans(toks: &[Tok], covered: &[(usize, usize)]) -> Vec<(usize, usize)> {
    let capw = |t: &Tok| is_cap(t.word) && !covered.iter().any(|&(s, e)| t.start < e && s < t.end);
    let mut out = vec![];
    let mut i = 0;
    while i < toks.len() {
        if !capw(&toks[i]) {
            i += 1;
            continue;
        }
        let mut start = i;
        let mut end = i;
        while !toks[end].closes {
            let mut k = end + 1;
            let mut n = 0;
            while k < toks.len() && n < 2 && !toks[k].opens && !toks[k].closes && CONNECTORS.contains(&toks[k].word.to_lowercase().as_str()) {
                k += 1;
                n += 1;
            }
            if k < toks.len() && capw(&toks[k]) && !toks[k].opens && !toks[k].sent_start {
                end = k;
            } else {
                break;
            }
        }
        i = end + 1;
        if toks[start].sent_start && COMMON_START.contains(&toks[start].word.to_lowercase().as_str()) {
            start += 1;
            while start <= end && !capw(&toks[start]) {
                start += 1;
            }
            if start > end {
                continue;
            }
        }
        if end - start < MAX_SPAN_WORDS {
            out.push((start, end));
        }
    }
    out
}

fn initials(toks: &[Tok]) -> String {
    toks.iter()
        .filter(|t| !CONNECTORS.contains(&t.word.to_lowercase().as_str()))
        .filter_map(|t| t.word.chars().next())
        .flat_map(char::to_uppercase)
        .collect()
}

/// Word n-grams (1..=4, not across clause punctuation) by name key:
/// (key, byte pos, surface, words, has an uppercase letter).
fn ngram_keys(text: &str, toks: &[Tok]) -> Vec<(String, usize, String, usize, bool)> {
    let mut out = vec![];
    for i in 0..toks.len() {
        for n in 1..=MAX_NGRAM {
            let j = i + n - 1;
            if j >= toks.len() || (j > i && (toks[j - 1].closes || toks[j].opens)) {
                break;
            }
            if toks[i].word.is_empty() || toks[j].word.is_empty() {
                continue;
            }
            let surface = &text[toks[i].start..toks[j].end];
            if let Some(k) = name_key(surface) {
                out.push((k, toks[i].start, strip_possessive(&collapse(surface)).to_string(), n, surface.chars().any(char::is_uppercase)));
            }
        }
    }
    out
}

/// Entity mentions and alias evidence in `text`. `seeds` are the name keys of
/// the user's `memory_entities` (names and aliases).
pub fn extract(text: &str, seeds: &HashSet<String>) -> Extracted {
    let (mut mentions, covered) = structured(text);
    let toks = tokens(text);
    let spans = cap_spans(&toks, &covered);
    let mut span_keys = vec![];
    for &(s, e) in &spans {
        let surface = strip_possessive(&collapse(&text[toks[s].start..toks[e].end])).to_string();
        let key = name_key(&surface);
        if let Some(k) = &key {
            mentions.push(Mention { key: k.clone(), kind: "name", surface, pos: toks[s].start });
        }
        span_keys.push(key);
    }
    let mut links = BTreeSet::new();
    for w in 0..spans.len().saturating_sub(1) {
        let ((a0, a1), (b0, b1)) = (spans[w], spans[w + 1]);
        let (Some(ka), Some(kb)) = (&span_keys[w], &span_keys[w + 1]) else { continue };
        let acronym = b0 == a1 + 1 && b0 == b1 && toks[b0].opens && is_acronym(toks[b0].word) && a1 > a0 && initials(&toks[a0..=a1]) == toks[b0].word;
        let gap: Vec<String> = toks[a1 + 1..b0].iter().map(|t| t.word.to_lowercase()).collect();
        let aka = matches!(gap.iter().map(String::as_str).collect::<Vec<_>>().as_slice(), ["aka"] | ["a.k.a"] | ["also", "known", "as"]);
        if acronym || aka {
            links.extend(pair(ka, kb));
        }
    }
    if !seeds.is_empty() {
        for (k, pos, surface, n, upper) in ngram_keys(text, &toks) {
            if (upper || n > 1) && seeds.contains(&k) {
                mentions.push(Mention { key: k, kind: "name", surface, pos });
            }
        }
    }
    let mut seen = HashSet::new();
    mentions.retain(|m| seen.insert((m.key.clone(), m.pos)));
    mentions.sort_by(|a, b| a.pos.cmp(&b.pos).then_with(|| a.key.cmp(&b.key)));
    Extracted { mentions, links }
}

/// One stored mention row: the mentions of one key in one target.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Row {
    key: String,
    kind: String,
    surface: String,
    count: i64,
    first_pos: i64,
}

fn aggregate(ms: &[Mention]) -> Vec<Row> {
    let mut by: BTreeMap<&str, (&str, BTreeMap<&str, (i64, usize)>, i64, usize)> = BTreeMap::new();
    for m in ms {
        let e = by.entry(m.key.as_str()).or_insert((m.kind, BTreeMap::new(), 0, m.pos));
        let s = e.1.entry(m.surface.as_str()).or_insert((0, m.pos));
        s.0 += 1;
        s.1 = s.1.min(m.pos);
        e.2 += 1;
        e.3 = e.3.min(m.pos);
    }
    by.into_iter()
        .map(|(key, (kind, surfaces, count, pos))| {
            let surface = surfaces.iter().max_by(|a, b| a.1 .0.cmp(&b.1 .0).then(b.1 .1.cmp(&a.1 .1))).map(|s| s.0.to_string()).unwrap_or_default();
            Row { key: key.into(), kind: kind.into(), surface, count, first_pos: pos as i64 }
        })
        .collect()
}

// ─── Incremental pass ───────────────────────────────────────────────────────

#[derive(Debug, Default, Clone, PartialEq, Serialize)]
pub struct PassReport {
    /// Queued targets read.
    pub targets: usize,
    /// Targets whose mentions changed.
    pub changed: usize,
    pub pages_written: usize,
    pub pages_deleted: usize,
}

fn has_properties(conn: &Connection) -> rusqlite::Result<bool> {
    conn.query_row("SELECT COUNT(*) FROM pragma_table_info('memory_entities') WHERE name = 'properties'", [], |r| r.get::<_, i64>(0)).map(|n| n > 0)
}

fn aliases_of(props: Option<&str>) -> Vec<String> {
    props
        .and_then(|p| serde_json::from_str::<serde_json::Value>(p).ok())
        .and_then(|v| v.get("aliases").and_then(|a| a.as_array()).cloned())
        .map(|a| a.iter().filter_map(|x| x.as_str()).map(collapse).filter(|s| !s.is_empty()).collect())
        .unwrap_or_default()
}

/// Name keys of the user's seed entities (names and aliases).
fn load_seeds(conn: &Connection, user: &str) -> rusqlite::Result<HashSet<String>> {
    let props = if has_properties(conn)? { "properties" } else { "NULL" };
    let mut stmt = conn.prepare(&format!("SELECT name, {props} FROM memory_entities WHERE user_id = ?1"))?;
    let rows = stmt.query_map(params![user], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))?;
    let mut out = HashSet::new();
    for r in rows {
        let (name, props) = r?;
        out.extend(name_key(&name));
        out.extend(aliases_of(props.as_deref()).iter().filter_map(|a| name_key(a)));
    }
    Ok(out)
}

/// The text a target is read as (documents and notes: title, blank line, body).
fn load_text(conn: &Connection, user: &str, ttype: &str, id: &str) -> rusqlite::Result<Option<String>> {
    let sql = match ttype {
        "fact" => "SELECT fact FROM memory_facts WHERE id = ?1 AND user_id = ?2 AND valid_until IS NULL",
        "observation" => "SELECT content FROM memory_observations WHERE id = ?1 AND user_id = ?2",
        "document" => "SELECT title || char(10) || char(10) || COALESCE(content, '') FROM memory_documents WHERE id = ?1 AND user_id = ?2",
        "note" => "SELECT title || char(10) || char(10) || content FROM memory_notes WHERE id = ?1 AND user_id = ?2",
        _ => return Ok(None),
    };
    conn.query_row(sql, params![id, user], |r| r.get::<_, Option<String>>(0)).optional().map(Option::flatten)
}

/// A seed entity's rows: its name and aliases, kind = its type.
fn seed_rows(conn: &Connection, user: &str, id: &str) -> rusqlite::Result<(Vec<Row>, BTreeSet<(String, String)>)> {
    let props = if has_properties(conn)? { "properties" } else { "NULL" };
    let row: Option<(String, String, Option<String>)> = conn
        .query_row(&format!("SELECT name, type, {props} FROM memory_entities WHERE id = ?1 AND user_id = ?2"), params![id, user], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .optional()?;
    let Some((name, etype, props)) = row else { return Ok((vec![], BTreeSet::new())) };
    let name = collapse(&name);
    let Some(nk) = name_key(&name) else { return Ok((vec![], BTreeSet::new())) };
    let mut rows = BTreeMap::new();
    rows.insert(nk.clone(), Row { key: nk.clone(), kind: etype.clone(), surface: name, count: 1, first_pos: 0 });
    let mut links = BTreeSet::new();
    for a in aliases_of(props.as_deref()) {
        if let Some(ak) = name_key(&a) {
            links.extend(pair(&nk, &ak));
            rows.entry(ak.clone()).or_insert(Row { key: ak, kind: etype.clone(), surface: a, count: 1, first_pos: 1 });
        }
    }
    Ok((rows.into_values().collect(), links))
}

/// Queue the user's targets whose keyword index matches `surface`.
fn requeue_matching(conn: &Connection, user: &str, surface: &str) -> rusqlite::Result<()> {
    let toks: Vec<String> = surface.split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty()).map(str::to_lowercase).collect();
    if toks.is_empty() {
        return Ok(());
    }
    let phrase = format!("\"{}\"", toks.join(" "));
    let mut stmt = conn.prepare(
        "SELECT target_type, target_id FROM memory_index_fts
         WHERE memory_index_fts MATCH ?1 AND user_id = ?2 AND target_type IN ('fact', 'observation', 'note', 'chunk') LIMIT ?3",
    )?;
    let hits: Vec<(String, String)> = stmt.query_map(params![phrase, user, SEED_REQUEUE_CAP], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
    for (t, id) in hits {
        let (t, id) = if t == "chunk" {
            match conn
                .query_row("SELECT source_id FROM memory_index_chunks WHERE id = ?1 AND scope = ?2 AND source_type = 'document'", params![id, user], |r| r.get(0))
                .optional()?
            {
                Some(doc) => ("document".to_string(), doc),
                None => continue,
            }
        } else {
            (t, id)
        };
        conn.execute("INSERT OR IGNORE INTO memory_entity_queue (user_id, target_type, target_id) VALUES (?1, ?2, ?3)", params![user, t, id])?;
    }
    Ok(())
}

fn stored_rows(conn: &Connection, user: &str, ttype: &str, id: &str) -> rusqlite::Result<(Vec<Row>, BTreeSet<(String, String)>)> {
    let mut stmt = conn.prepare(
        "SELECT key, kind, surface, count, first_pos FROM memory_entity_mentions
         WHERE user_id = ?1 AND target_type = ?2 AND target_id = ?3 ORDER BY key",
    )?;
    let rows = stmt
        .query_map(params![user, ttype, id], |r| Ok(Row { key: r.get(0)?, kind: r.get(1)?, surface: r.get(2)?, count: r.get(3)?, first_pos: r.get(4)? }))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut stmt = conn.prepare("SELECT key_a, key_b FROM memory_entity_links WHERE user_id = ?1 AND target_type = ?2 AND target_id = ?3")?;
    let links = stmt.query_map(params![user, ttype, id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<BTreeSet<_>, _>>()?;
    Ok((rows, links))
}

/// Re-read one target. Returns whether its mentions or links changed.
fn process_target(conn: &Connection, user: &str, ttype: &str, id: &str, seeds: &mut HashMap<String, HashSet<String>>) -> rusqlite::Result<bool> {
    let (rows, links) = if ttype == "entity" {
        seed_rows(conn, user, id)?
    } else {
        match load_text(conn, user, ttype, id)? {
            Some(text) => {
                if !seeds.contains_key(user) {
                    seeds.insert(user.to_string(), load_seeds(conn, user)?);
                }
                let ex = extract(&text, &seeds[user]);
                (aggregate(&ex.mentions), ex.links)
            }
            None => (vec![], BTreeSet::new()),
        }
    };
    let (old_rows, old_links) = stored_rows(conn, user, ttype, id)?;
    if old_rows == rows && old_links == links {
        return Ok(false);
    }
    conn.execute("DELETE FROM memory_entity_mentions WHERE user_id = ?1 AND target_type = ?2 AND target_id = ?3", params![user, ttype, id])?;
    conn.execute("DELETE FROM memory_entity_links WHERE user_id = ?1 AND target_type = ?2 AND target_id = ?3", params![user, ttype, id])?;
    for r in &rows {
        conn.execute(
            "INSERT INTO memory_entity_mentions (user_id, key, kind, surface, target_type, target_id, count, first_pos)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![user, r.key, r.kind, r.surface, ttype, id, r.count, r.first_pos],
        )?;
    }
    for (a, b) in &links {
        conn.execute(
            "INSERT OR IGNORE INTO memory_entity_links (user_id, key_a, key_b, target_type, target_id) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![user, a, b, ttype, id],
        )?;
    }
    let mut dirty: BTreeSet<&str> = old_rows.iter().chain(rows.iter()).map(|r| r.key.as_str()).collect();
    dirty.extend(old_links.iter().chain(links.iter()).flat_map(|(a, b)| [a.as_str(), b.as_str()]));
    for k in dirty {
        conn.execute("INSERT OR IGNORE INTO memory_entity_dirty (user_id, key) VALUES (?1, ?2)", params![user, k])?;
    }
    if ttype == "entity" {
        // New or changed seed names: texts that mention them in any case
        // get re-read with the new seed set.
        seeds.remove(user);
        let old: BTreeSet<&str> = old_rows.iter().map(|r| r.surface.as_str()).collect();
        let new: BTreeSet<&str> = rows.iter().map(|r| r.surface.as_str()).collect();
        for s in old.symmetric_difference(&new) {
            requeue_matching(conn, user, s)?;
        }
    }
    Ok(true)
}

/// One pass: read up to `batch` queued targets, then regenerate the pages of
/// the entities whose mentions changed. One transaction.
pub fn run_pass(conn: &Connection, batch: usize) -> rusqlite::Result<PassReport> {
    let tx = conn.unchecked_transaction()?;
    let mut report = PassReport::default();
    let queued: Vec<(String, String, String)> = {
        let mut stmt = tx.prepare(
            "SELECT user_id, target_type, target_id FROM memory_entity_queue ORDER BY queued_at, user_id, target_type, target_id LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![batch as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<_, _>>()?;
        rows
    };
    let mut seeds = HashMap::new();
    for (user, ttype, id) in &queued {
        report.targets += 1;
        if process_target(&tx, user, ttype, id, &mut seeds)? {
            report.changed += 1;
        }
        tx.execute("DELETE FROM memory_entity_queue WHERE user_id = ?1 AND target_type = ?2 AND target_id = ?3", params![user, ttype, id])?;
    }
    let users: Vec<String> = {
        let mut stmt = tx.prepare("SELECT DISTINCT user_id FROM memory_entity_dirty ORDER BY user_id")?;
        let rows = stmt.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
        rows
    };
    for user in users {
        rebuild_user(&tx, &user, false, &mut report)?;
    }
    tx.commit()?;
    Ok(report)
}

/// Regenerate every page of one user from the stored mentions.
pub fn rebuild_all(conn: &Connection, user: &str) -> rusqlite::Result<PassReport> {
    let tx = conn.unchecked_transaction()?;
    let mut report = PassReport::default();
    rebuild_user(&tx, user, true, &mut report)?;
    tx.commit()?;
    Ok(report)
}

/// Queue every target of one user (e.g. after an extractor change).
pub fn requeue_user(conn: &Connection, user: &str) -> rusqlite::Result<usize> {
    let mut n = 0;
    for (t, sql) in [
        ("entity", "SELECT id FROM memory_entities WHERE user_id = ?1"),
        ("fact", "SELECT id FROM memory_facts WHERE user_id = ?1 AND valid_until IS NULL"),
        ("observation", "SELECT id FROM memory_observations WHERE user_id = ?1"),
        ("document", "SELECT id FROM memory_documents WHERE user_id = ?1"),
        ("note", "SELECT id FROM memory_notes WHERE user_id = ?1"),
    ] {
        n += conn.execute(&format!("INSERT OR IGNORE INTO memory_entity_queue (user_id, target_type, target_id) SELECT ?1, '{t}', id FROM ({sql})"), params![user])?;
    }
    Ok(n)
}

pub fn entity_id(user: &str, key: &str) -> String {
    let mut h = Sha256::new();
    h.update(user.as_bytes());
    h.update([0u8]);
    h.update(key.as_bytes());
    format!("ent_{}", &hex::encode(h.finalize())[..24])
}

/// What one resolved entity is made of (from the user's mention rows).
#[derive(Default)]
struct Agg {
    /// (target type, id) -> (mentions, first pos); seed rows excluded.
    targets: BTreeMap<(String, String), (i64, i64)>,
    surfaces: BTreeMap<String, i64>,
    kinds: BTreeMap<String, i64>,
    /// Seed entity id -> (name, type).
    seeds: BTreeMap<String, (String, String)>,
    mentions: i64,
}

impl Agg {
    fn qualifies(&self) -> bool {
        !self.seeds.is_empty() || self.targets.len() >= MIN_TARGETS
    }
    fn name(&self) -> String {
        if let Some((name, _)) = self.seeds.values().next() {
            return name.clone();
        }
        // Acronyms abbreviate: prefer a spelled-out surface when there is one.
        let spelled = self.surfaces.keys().any(|s| !is_acronym(s));
        self.surfaces
            .iter()
            .filter(|(s, _)| !spelled || !is_acronym(s))
            .max_by(|a, b| a.1.cmp(b.1).then(a.0.len().cmp(&b.0.len())).then(b.0.cmp(a.0)))
            .map(|s| s.0.clone())
            .unwrap_or_default()
    }
    fn kind(&self) -> String {
        if let Some((_, t)) = self.seeds.values().find(|(_, t)| !t.eq_ignore_ascii_case("general")) {
            return t.clone();
        }
        self.kinds.iter().max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0))).map(|k| k.0.clone()).unwrap_or_else(|| "name".into())
    }
}

struct Uf(Vec<usize>);
impl Uf {
    fn find(&mut self, i: usize) -> usize {
        let mut r = i;
        while self.0[r] != r {
            r = self.0[r];
        }
        let mut c = i;
        while self.0[c] != r {
            let n = self.0[c];
            self.0[c] = r;
            c = n;
        }
        r
    }
    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.0[ra.max(rb)] = ra.min(rb);
        }
    }
}

/// Re-resolve one user's keys and regenerate the pages of affected entities
/// (all of them when `all`).
fn rebuild_user(conn: &Connection, user: &str, all: bool, report: &mut PassReport) -> rusqlite::Result<()> {
    struct M {
        key: String,
        kind: String,
        surface: String,
        ttype: String,
        tid: String,
        count: i64,
        pos: i64,
    }
    let rows: Vec<M> = {
        let mut stmt = conn.prepare(
            "SELECT key, kind, surface, target_type, target_id, count, first_pos FROM memory_entity_mentions
             WHERE user_id = ?1 ORDER BY key, target_type, target_id",
        )?;
        let rows = stmt
            .query_map(params![user], |r| {
                Ok(M { key: r.get(0)?, kind: r.get(1)?, surface: r.get(2)?, ttype: r.get(3)?, tid: r.get(4)?, count: r.get(5)?, pos: r.get(6)? })
            })?
            .collect::<Result<_, _>>()?;
        rows
    };
    let links: Vec<(String, String)> = {
        let mut stmt = conn.prepare("SELECT DISTINCT key_a, key_b FROM memory_entity_links WHERE user_id = ?1 ORDER BY 1, 2")?;
        let rows = stmt.query_map(params![user], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
        rows
    };
    // Union-find over keys with mentions; links only join keys that are mentioned.
    let keys: Vec<&str> = rows.iter().map(|m| m.key.as_str()).collect::<BTreeSet<_>>().into_iter().collect();
    let idx: HashMap<&str, usize> = keys.iter().enumerate().map(|(i, k)| (*k, i)).collect();
    let mut uf = Uf((0..keys.len()).collect());
    for (a, b) in &links {
        if let (Some(&ia), Some(&ib)) = (idx.get(a.as_str()), idx.get(b.as_str())) {
            uf.union(ia, ib);
        }
    }
    let seeded: HashSet<&str> = rows.iter().filter(|m| m.ttype == "entity").map(|m| m.key.as_str()).collect();
    let mut members: BTreeMap<usize, Vec<&str>> = BTreeMap::new();
    for (i, k) in keys.iter().enumerate() {
        members.entry(uf.find(i)).or_default().push(k);
    }
    let mut new_map: HashMap<String, String> = HashMap::new();
    for ks in members.values() {
        // Canonical key: the smallest seeded key, else the smallest key.
        let canon = ks.iter().filter(|k| seeded.contains(**k)).min().or_else(|| ks.iter().min()).unwrap();
        let id = entity_id(user, canon);
        for k in ks {
            new_map.insert(k.to_string(), id.clone());
        }
    }
    let mut aggs: HashMap<String, Agg> = HashMap::new();
    let mut by_target: HashMap<(String, String), BTreeSet<String>> = HashMap::new();
    for m in &rows {
        let id = &new_map[&m.key];
        let a = aggs.entry(id.clone()).or_default();
        if m.ttype == "entity" {
            // Seed rows: the name (pos 0) names the entity, aliases (pos 1) are listed.
            if m.pos == 0 {
                a.seeds.entry(m.tid.clone()).or_insert((m.surface.clone(), m.kind.clone()));
            }
            a.surfaces.entry(m.surface.clone()).or_default();
            continue;
        }
        let t = a.targets.entry((m.ttype.clone(), m.tid.clone())).or_insert((0, i64::MAX));
        t.0 += m.count;
        t.1 = t.1.min(m.pos);
        *a.surfaces.entry(m.surface.clone()).or_default() += m.count;
        *a.kinds.entry(m.kind.clone()).or_default() += m.count;
        a.mentions += m.count;
        by_target.entry((m.ttype.clone(), m.tid.clone())).or_default().insert(id.clone());
    }

    let old_map: HashMap<String, String> = {
        let mut stmt = conn.prepare("SELECT key, entity_id FROM memory_entity_keys WHERE user_id = ?1")?;
        let rows = stmt.query_map(params![user], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
        rows
    };
    let dirty: Vec<String> = if all {
        new_map.keys().chain(old_map.keys()).cloned().collect()
    } else {
        let mut stmt = conn.prepare("SELECT key FROM memory_entity_dirty WHERE user_id = ?1")?;
        let rows = stmt.query_map(params![user], |r| r.get(0))?.collect::<Result<_, _>>()?;
        rows
    };
    let mut affected: BTreeSet<String> = BTreeSet::new();
    for k in &dirty {
        affected.extend(new_map.get(k).cloned());
        affected.extend(old_map.get(k).cloned());
    }
    for (k, e) in &old_map {
        if new_map.get(k) != Some(e) {
            affected.insert(e.clone());
            affected.extend(new_map.get(k).cloned());
        }
    }
    for (k, e) in &new_map {
        if !old_map.contains_key(k) {
            affected.insert(e.clone());
        }
    }
    if all {
        let mut stmt = conn.prepare("SELECT id FROM memory_entity_pages WHERE user_id = ?1")?;
        let ids: Vec<String> = stmt.query_map(params![user], |r| r.get(0))?.collect::<Result<_, _>>()?;
        affected.extend(ids);
    }
    // An entity whose name or page status changed shows up in its
    // co-mentioned entities' related lists: rebuild those too.
    let stored: HashMap<String, String> = {
        let mut stmt = conn.prepare("SELECT id, name FROM memory_entity_pages WHERE user_id = ?1")?;
        let rows = stmt.query_map(params![user], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
        rows
    };
    for e in affected.clone() {
        let now = aggs.get(&e).filter(|a| a.qualifies()).map(Agg::name);
        if stored.get(&e) != now.as_ref() {
            if let Some(a) = aggs.get(&e) {
                for t in a.targets.keys() {
                    affected.extend(by_target.get(t).into_iter().flatten().cloned());
                }
            }
        }
    }

    for (k, e) in &old_map {
        match new_map.get(k) {
            None => {
                conn.execute("DELETE FROM memory_entity_keys WHERE user_id = ?1 AND key = ?2", params![user, k])?;
            }
            Some(n) if n != e => {
                conn.execute("UPDATE memory_entity_keys SET entity_id = ?3 WHERE user_id = ?1 AND key = ?2", params![user, k, n])?;
            }
            _ => {}
        }
    }
    for (k, n) in &new_map {
        if !old_map.contains_key(k) {
            conn.execute("INSERT INTO memory_entity_keys (user_id, key, entity_id) VALUES (?1, ?2, ?3)", params![user, k, n])?;
        }
    }

    for e in &affected {
        match aggs.get(e).filter(|a| a.qualifies()) {
            Some(a) => {
                let page = build_page(conn, user, e, a, &aggs, &by_target)?;
                if write_page(conn, user, &page)? {
                    report.pages_written += 1;
                }
            }
            None => {
                report.pages_deleted += conn.execute("DELETE FROM memory_entity_pages WHERE id = ?1 AND user_id = ?2", params![e, user])?;
            }
        }
    }
    conn.execute("DELETE FROM memory_entity_dirty WHERE user_id = ?1", params![user])?;
    Ok(())
}

// ─── Pages ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PageItem {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub text: String,
    pub mentions: i64,
    /// Byte offset of the first mention in the target's text.
    pub pos: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Related {
    pub id: String,
    pub name: String,
    /// Targets that mention both.
    pub shared: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntityPage {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub aliases: Vec<String>,
    pub memory_entity_id: Option<String>,
    pub mention_count: i64,
    pub target_count: i64,
    pub facts: Vec<PageItem>,
    pub observations: Vec<PageItem>,
    pub documents: Vec<PageItem>,
    pub notes: Vec<PageItem>,
    pub related: Vec<Related>,
}

/// Up to `width` chars of `text` around byte offset `pos`, on one line.
fn snippet(text: &str, pos: usize, width: usize) -> String {
    let mut pos = pos.min(text.len());
    while !text.is_char_boundary(pos) {
        pos -= 1;
    }
    let start = text[..pos].char_indices().rev().nth(width / 3).map(|x| x.0).unwrap_or(0);
    let body: String = text[start..].chars().take(width).collect();
    let more = text[start..].chars().count() > width;
    format!("{}{}{}", if start > 0 { "…" } else { "" }, collapse(&body), if more { "…" } else { "" })
}

/// (title, body) of a document or note; the stored position is in `title\n\nbody`.
fn titled(conn: &Connection, user: &str, ttype: &str, id: &str) -> rusqlite::Result<Option<(String, String)>> {
    let sql = match ttype {
        "document" => "SELECT title, COALESCE(content, '') FROM memory_documents WHERE id = ?1 AND user_id = ?2",
        "note" => "SELECT title, content FROM memory_notes WHERE id = ?1 AND user_id = ?2",
        _ => return Ok(None),
    };
    conn.query_row(sql, params![id, user], |r| Ok((r.get(0)?, r.get(1)?))).optional()
}

fn body_pos(title: &str, pos: i64) -> usize {
    (pos.max(0) as usize).saturating_sub(title.len() + 2)
}

/// A passage of a linked document or note around the entity's first mention.
pub fn linked_passage(conn: &Connection, user: &str, ttype: &str, id: &str, pos: i64, width: usize) -> rusqlite::Result<Option<(String, String)>> {
    Ok(titled(conn, user, ttype, id)?.map(|(title, body)| {
        let p = body_pos(&title, pos);
        (title, snippet(&body, p, width))
    }))
}

fn build_page(
    conn: &Connection,
    user: &str,
    id: &str,
    a: &Agg,
    aggs: &HashMap<String, Agg>,
    by_target: &HashMap<(String, String), BTreeSet<String>>,
) -> rusqlite::Result<EntityPage> {
    let name = a.name();
    let mut aliases: Vec<(&String, &i64)> = a.surfaces.iter().filter(|(s, _)| !s.eq_ignore_ascii_case(&name)).collect();
    aliases.sort_by(|x, y| y.1.cmp(x.1).then(x.0.cmp(y.0)));
    let mut seen = HashSet::new();
    let aliases: Vec<String> =
        aliases.into_iter().map(|(s, _)| s.clone()).filter(|s| seen.insert(s.to_lowercase())).take(PAGE_ALIASES).collect();
    let mut ranked: Vec<(&(String, String), &(i64, i64))> = a.targets.iter().collect();
    ranked.sort_by(|x, y| y.1 .0.cmp(&x.1 .0).then(x.0.cmp(y.0)));
    let (mut facts, mut observations, mut documents, mut notes) = (vec![], vec![], vec![], vec![]);
    for ((t, tid), (count, pos)) in ranked {
        let item = |title: Option<String>, text: String| PageItem { id: tid.clone(), title, text, mentions: *count, pos: *pos };
        match t.as_str() {
            "fact" | "observation" => {
                let list = if t == "fact" { &mut facts } else { &mut observations };
                if list.len() >= if t == "fact" { PAGE_FACTS } else { PAGE_OBSERVATIONS } {
                    continue;
                }
                if let Some(text) = load_text(conn, user, t, tid)? {
                    list.push(item(None, clip(&text, ITEM_CHARS)));
                }
            }
            "document" | "note" => {
                let list = if t == "document" { &mut documents } else { &mut notes };
                if list.len() >= if t == "document" { PAGE_DOCUMENTS } else { PAGE_NOTES } {
                    continue;
                }
                if let Some((title, body)) = titled(conn, user, t, tid)? {
                    list.push(item(Some(clip(&title, 120)), snippet(&body, body_pos(&title, *pos), SNIPPET_CHARS)));
                }
            }
            _ => {}
        }
    }
    let mut shared: BTreeMap<&str, i64> = BTreeMap::new();
    for t in a.targets.keys() {
        for o in by_target.get(t).into_iter().flatten() {
            if o != id && aggs.get(o).is_some_and(Agg::qualifies) {
                *shared.entry(o.as_str()).or_default() += 1;
            }
        }
    }
    let mut related: Vec<Related> = shared.into_iter().map(|(o, n)| Related { id: o.to_string(), name: aggs[o].name(), shared: n }).collect();
    related.sort_by(|x, y| y.shared.cmp(&x.shared).then(x.name.cmp(&y.name)).then(x.id.cmp(&y.id)));
    related.truncate(PAGE_RELATED);
    Ok(EntityPage {
        id: id.to_string(),
        name,
        kind: a.kind(),
        aliases,
        memory_entity_id: a.seeds.keys().next().cloned(),
        mention_count: a.mentions,
        target_count: a.targets.len() as i64,
        facts,
        observations,
        documents,
        notes,
        related,
    })
}

/// The page as Markdown.
pub fn render_text(p: &EntityPage) -> String {
    let mut s = format!("# {}\n\n", p.name);
    s.push_str(&format!("type: {} · mentioned in {} items ({} mentions)\n", p.kind, p.target_count, p.mention_count));
    if !p.aliases.is_empty() {
        s.push_str(&format!("aliases: {}\n", p.aliases.join(", ")));
    }
    let section = |s: &mut String, head: &str, items: &[PageItem], ttype: &str| {
        if items.is_empty() {
            return;
        }
        s.push_str(&format!("\n## {head}\n"));
        for i in items {
            match &i.title {
                Some(t) => s.push_str(&format!("- {t}: {} [{ttype}:{}]\n", i.text, i.id)),
                None => s.push_str(&format!("- {} [{ttype}:{}]\n", i.text, i.id)),
            }
        }
    };
    section(&mut s, "Facts", &p.facts, "fact");
    section(&mut s, "Observations", &p.observations, "observation");
    section(&mut s, "Documents", &p.documents, "document");
    section(&mut s, "Notes", &p.notes, "note");
    if !p.related.is_empty() {
        s.push_str("\n## Related\n");
        for r in &p.related {
            s.push_str(&format!("- {} ({} shared) [entity:{}]\n", r.name, r.shared, r.id));
        }
    }
    s
}

/// A compact page for a model prompt, at most `max_chars`: header, facts,
/// one observation, related names. Documents are left to the caller (they are
/// followed as links). Returns the text and the (type, id) items it shows.
pub fn render_compact(p: &EntityPage, max_chars: usize) -> (String, Vec<(String, String)>) {
    let mut head = format!("- (entity) {} [{}]", p.name, p.kind);
    if !p.aliases.is_empty() {
        head.push_str(&format!(" aka {}", p.aliases.iter().take(4).cloned().collect::<Vec<_>>().join(", ")));
    }
    head.push_str(&format!("; in {} items\n", p.target_count));
    let mut out = clip(&head, max_chars.saturating_sub(1));
    out.push('\n');
    let mut shown = vec![];
    let lines = p
        .facts
        .iter()
        .take(3)
        .map(|f| ("fact", f))
        .chain(p.observations.iter().take(1).map(|o| ("observation", o)));
    for (t, i) in lines {
        let line = format!("  · {}\n", clip(&i.text, 200));
        if out.len() + line.len() > max_chars {
            return (out, shown);
        }
        out.push_str(&line);
        shown.push((t.to_string(), i.id.clone()));
    }
    if !p.related.is_empty() {
        let line = format!("  · related: {}\n", p.related.iter().take(4).map(|r| r.name.as_str()).collect::<Vec<_>>().join(", "));
        if out.len() + line.len() <= max_chars {
            out.push_str(&line);
        }
    }
    (out, shown)
}

fn write_page(conn: &Connection, user: &str, p: &EntityPage) -> rusqlite::Result<bool> {
    let page_json = serde_json::to_string(p).unwrap_or_default();
    let hash = hex::encode(Sha256::digest(page_json.as_bytes()));
    let old: Option<String> = conn
        .query_row("SELECT page_hash FROM memory_entity_pages WHERE id = ?1 AND user_id = ?2", params![p.id, user], |r| r.get(0))
        .optional()?;
    if old.as_deref() == Some(hash.as_str()) {
        return Ok(false);
    }
    conn.execute(
        "INSERT INTO memory_entity_pages (id, user_id, name, kind, aliases, memory_entity_id, mention_count, target_count, page_json, page_text, page_hash, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, CURRENT_TIMESTAMP)
         ON CONFLICT(id) DO UPDATE SET name = ?3, kind = ?4, aliases = ?5, memory_entity_id = ?6, mention_count = ?7,
           target_count = ?8, page_json = ?9, page_text = ?10, page_hash = ?11, updated_at = CURRENT_TIMESTAMP
         WHERE memory_entity_pages.user_id = ?2",
        params![
            p.id,
            user,
            p.name,
            p.kind,
            serde_json::to_string(&p.aliases).unwrap_or_default(),
            p.memory_entity_id,
            p.mention_count,
            p.target_count,
            page_json,
            render_text(p),
            hash
        ],
    )?;
    Ok(true)
}

// ─── Reads ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct PageSummary {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub aliases: Vec<String>,
    pub mention_count: i64,
    pub target_count: i64,
    pub updated_at: String,
}

fn like_escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('%');
    for c in s.chars() {
        if matches!(c, '%' | '_' | '\\') {
            o.push('\\');
        }
        o.push(c);
    }
    o.push('%');
    o
}

/// The user's pages matching `q` (exact name / alias key first, then name or
/// alias substring, then page text), or the most mentioned when `q` is empty.
pub fn search_pages(conn: &Connection, user: &str, q: Option<&str>, limit: usize) -> rusqlite::Result<Vec<PageSummary>> {
    let q = q.map(str::trim).filter(|q| !q.is_empty());
    let key = q.and_then(name_key).unwrap_or_default();
    let like = q.map(like_escape);
    let mut stmt = conn.prepare(
        "SELECT id, name, kind, aliases, mention_count, target_count, COALESCE(updated_at, ''),
                CASE WHEN id IN (SELECT entity_id FROM memory_entity_keys WHERE user_id = ?1 AND key = ?2) THEN 3
                     WHEN name LIKE ?3 ESCAPE '\\' OR aliases LIKE ?3 ESCAPE '\\' THEN 2 ELSE 1 END AS score
         FROM memory_entity_pages
         WHERE user_id = ?1 AND (?3 IS NULL
                OR id IN (SELECT entity_id FROM memory_entity_keys WHERE user_id = ?1 AND key = ?2)
                OR name LIKE ?3 ESCAPE '\\' OR aliases LIKE ?3 ESCAPE '\\' OR page_text LIKE ?3 ESCAPE '\\')
         ORDER BY score DESC, target_count DESC, name, id LIMIT ?4",
    )?;
    let rows = stmt.query_map(params![user, key, like, limit as i64], |r| {
        Ok(PageSummary {
            id: r.get(0)?,
            name: r.get(1)?,
            kind: r.get(2)?,
            aliases: serde_json::from_str(&r.get::<_, String>(3)?).unwrap_or_default(),
            mention_count: r.get(4)?,
            target_count: r.get(5)?,
            updated_at: r.get(6)?,
        })
    })?;
    rows.collect()
}

pub fn load_page(conn: &Connection, user: &str, id: &str) -> rusqlite::Result<Option<EntityPage>> {
    let raw: Option<String> =
        conn.query_row("SELECT page_json FROM memory_entity_pages WHERE id = ?1 AND user_id = ?2", params![id, user], |r| r.get(0)).optional()?;
    Ok(raw.and_then(|s| serde_json::from_str(&s).ok()))
}

#[derive(Debug, Clone, Serialize)]
pub struct MentionLink {
    pub target_type: String,
    pub target_id: String,
    pub mentions: i64,
}

/// Every target that mentions the entity, most mentions first.
pub fn mention_links(conn: &Connection, user: &str, id: &str, limit: usize) -> rusqlite::Result<Vec<MentionLink>> {
    let mut stmt = conn.prepare(
        "SELECT m.target_type, m.target_id, SUM(m.count) AS n FROM memory_entity_mentions m
         JOIN memory_entity_keys k ON k.user_id = m.user_id AND k.key = m.key
         WHERE m.user_id = ?1 AND k.entity_id = ?2 AND m.target_type != 'entity'
         GROUP BY m.target_type, m.target_id ORDER BY n DESC, m.target_type, m.target_id LIMIT ?3",
    )?;
    let rows = stmt.query_map(params![user, id, limit as i64], |r| Ok(MentionLink { target_type: r.get(0)?, target_id: r.get(1)?, mentions: r.get(2)? }))?;
    rows.collect()
}

/// Pages of the entities `text` names (in order of first mention), at most
/// `max`. A one-word lowercase match counts only for seed entities, so common
/// words in a goal do not pull in pages.
pub fn pages_for_text(conn: &Connection, user: &str, text: &str, max: usize) -> rusqlite::Result<Vec<EntityPage>> {
    let toks = tokens(text);
    let mut cands: Vec<(usize, String, bool)> = structured(text).0.into_iter().map(|m| (m.pos, m.key, true)).collect();
    cands.extend(ngram_keys(text, &toks).into_iter().map(|(k, pos, _, n, upper)| (pos, k, upper || n > 1)));
    cands.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.len().cmp(&a.1.len())));
    let mut stmt = conn.prepare(
        "SELECT p.id, p.memory_entity_id IS NOT NULL, p.target_count FROM memory_entity_keys k
         JOIN memory_entity_pages p ON p.id = k.entity_id AND p.user_id = k.user_id
         WHERE k.user_id = ?1 AND k.key = ?2",
    )?;
    let mut ids: Vec<String> = vec![];
    for (_, key, strong) in cands {
        let hit: Option<(String, bool, i64)> = stmt.query_row(params![user, key], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional()?;
        if let Some((id, seeded, _)) = hit {
            if (strong || seeded) && !ids.contains(&id) {
                ids.push(id);
            }
        }
        if ids.len() >= max {
            break;
        }
    }
    let mut out = vec![];
    for id in ids {
        out.extend(load_page(conn, user, &id)?);
    }
    Ok(out)
}

// ─── Routes ─────────────────────────────────────────────────────────────────

pub fn memory_entity_pages_router() -> Router<Arc<AppState>> {
    Router::new().route("/memory/entities/:id", get(get_route))
}

async fn get_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(id): Path<String>) -> Response {
    let db = state.db.clone();
    let res = tokio::task::spawn_blocking(move || -> rusqlite::Result<_> {
        let conn = db.connect()?;
        let Some(page) = load_page(&conn, &user.user_id, &id)? else { return Ok(None) };
        let links = mention_links(&conn, &user.user_id, &id, 200)?;
        Ok(Some((page, links)))
    })
    .await;
    match res {
        Ok(Ok(Some((page, links)))) => {
            let text = render_text(&page);
            Json(json!({ "page": page, "text": text, "mentions": links })).into_response()
        }
        Ok(Ok(None)) => (StatusCode::NOT_FOUND, Json(json!({ "error": "not_found" }))).into_response(),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

#[cfg(test)]
mod tests;
