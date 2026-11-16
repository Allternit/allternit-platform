//! An in-process `files/2` runtime for the sync-root test: just enough of the
//! contract (roots, list, stat, ranged content, changes, upload sessions with
//! keep-both conflicts, mkdir, move, delete, pin, wake consent) to drive the
//! helper through Windows. It is the network edge; the helper under test is real.

#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

pub const COMPUTER: &str = "cmp_test";
pub const TICKET: &str = "wt_test";
const AT: &str = "2026-11-15T10:00:00Z";

pub struct FakeItem {
    pub id: String,
    pub root: String,
    pub parent: Option<String>,
    pub name: String,
    pub dir: bool,
    pub data: Vec<u8>,
    pub mv: u64,
    pub trashed: bool,
}

pub struct Root {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub item: String,
    pub protected: bool,
}

#[derive(Default)]
pub struct State {
    pub items: BTreeMap<String, FakeItem>,
    pub roots: Vec<Root>,
    /// (root id, change JSON); seq = position + 1.
    pub changes: Vec<(String, Value)>,
    pub chunks: HashMap<String, Vec<u8>>,
    pub sessions: HashMap<String, (Value, Vec<Value>)>,
    pub asleep: bool,
    pub consent_calls: u64,
    /// (offset, length) of every ranged content read.
    pub ranges: Vec<(u64, u64)>,
    pub ticketed_requests: u64,
    pub content_refused: u64,
    next: u64,
}

fn hash(b: &[u8]) -> String {
    blake3::hash(b).to_hex().to_string()
}

impl State {
    fn id(&mut self, p: &str) -> String {
        self.next += 1;
        format!("{p}_{}", self.next)
    }

    pub fn rev(it: &FakeItem) -> String {
        if it.dir {
            format!("{}-dir", it.mv)
        } else {
            format!("{}-{}", it.mv, hash(&it.data))
        }
    }

    pub fn item_json(it: &FakeItem) -> Value {
        let mut v = json!({
            "id": it.id, "root_id": it.root, "name": it.name,
            "kind": if it.dir { "folder" } else { "file" },
            "size": if it.dir { 0 } else { it.data.len() },
            "rev": Self::rev(it), "meta_version": it.mv,
            "created_at": AT, "modified_at": AT,
            "permissions": {"list": true, "read": true, "write": true, "delete": true, "share": true,
                            "comment": true, "agent_process": false, "transfer": true, "execute": false}
        });
        if let Some(p) = &it.parent {
            v["parent_id"] = json!(p);
        }
        if !it.dir {
            v["content_hash"] = json!(hash(&it.data));
        }
        v
    }

    fn cursor(&self, root: &str) -> String {
        let n = self.changes.iter().enumerate().filter(|(_, (r, _))| r == root).map(|(i, _)| i + 1).max().unwrap_or(0);
        format!("c1.{n}")
    }

    fn record(&mut self, op: &str, id: &str) {
        let it = &self.items[id];
        let seq = self.changes.len() + 1;
        let c = json!({"seq": seq, "op": op, "item_id": id, "item": Self::item_json(it), "at": AT,
                       "by": {"kind": "owner", "id": "usr_test"}});
        let root = it.root.clone();
        self.changes.push((root, c));
    }

    fn freshness(&self) -> Value {
        if self.asleep {
            json!({"served_by": "snapshot", "computer_state": "asleep", "snapshot_at": AT})
        } else {
            json!({"served_by": "live", "computer_state": "awake"})
        }
    }

    pub fn add(&mut self, root: &str, parent: Option<&str>, name: &str, dir: bool, data: &[u8]) -> String {
        let id = self.id("it");
        self.items.insert(
            id.clone(),
            FakeItem { id: id.clone(), root: root.into(), parent: parent.map(Into::into), name: name.into(), dir, data: data.to_vec(), mv: 1, trashed: false },
        );
        if parent.is_some() {
            self.record("created", &id);
        }
        id
    }

    pub fn modify(&mut self, id: &str, data: &[u8]) {
        let it = self.items.get_mut(id).unwrap();
        it.data = data.to_vec();
        it.mv += 1;
        self.record("modified", id);
    }

    pub fn rename(&mut self, id: &str, name: &str) {
        let it = self.items.get_mut(id).unwrap();
        it.name = name.into();
        it.mv += 1;
        self.record("renamed", id);
    }

    pub fn trash(&mut self, id: &str) {
        self.items.get_mut(id).unwrap().trashed = true;
        self.record("trashed", id);
    }

    pub fn find(&self, parent: &str, name: &str) -> Option<&FakeItem> {
        self.items.values().find(|i| !i.trashed && i.parent.as_deref() == Some(parent) && i.name == name)
    }

    fn taken(&self, parent: &str, name: &str) -> bool {
        self.find(parent, name).is_some()
    }

    fn free_name(&self, parent: &str, name: &str) -> String {
        if !self.taken(parent, name) {
            return name.into();
        }
        let (stem, ext) = match name.rfind('.') {
            Some(i) if i > 0 => (&name[..i], &name[i..]),
            _ => (name, ""),
        };
        (2..).map(|n| format!("{stem} ({n}){ext}")).find(|c| !self.taken(parent, c)).unwrap()
    }
}

pub struct Fake {
    pub port: u16,
    pub state: Arc<Mutex<State>>,
}

pub struct Req {
    method: String,
    path: String,
    query: HashMap<String, String>,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn read_req(stream: &mut TcpStream) -> Option<Req> {
    let mut r = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    r.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();
    let mut headers = HashMap::new();
    loop {
        let mut h = String::new();
        r.read_line(&mut h).ok()?;
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    let len: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).ok()?;
    let (path, q) = target.split_once('?').unwrap_or((&target, ""));
    let query = q.split('&').filter(|x| !x.is_empty()).filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.to_string(), unescape(v))).collect();
    Some(Req { method, path: path.to_string(), query, headers, body })
}

fn respond(stream: &mut TcpStream, status: u16, ctype: &str, extra: &[(&str, String)], body: &[u8]) {
    let mut head = format!("HTTP/1.1 {status} X\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n", body.len());
    for (k, v) in extra {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
}

fn err(code: &str, status: u16, extra: Value) -> (u16, Value) {
    let mut v = json!({"error": code, "code": code, "message": code});
    if let Value::Object(m) = extra {
        for (k, x) in m {
            v[k] = x;
        }
    }
    (status, v)
}

fn session_status(sid: &str, st: &State, req: &Value, refs: &[Value]) -> Value {
    let need: Vec<String> = refs.iter().map(|r| r["hash"].as_str().unwrap().to_string()).filter(|h| !st.chunks.contains_key(h)).collect();
    let received: u64 = refs.iter().filter(|r| st.chunks.contains_key(r["hash"].as_str().unwrap())).map(|r| r["size"].as_u64().unwrap()).sum();
    json!({"session_id": sid, "state": "open", "need": need, "received_bytes": received,
           "total_bytes": req["size"], "expires_at": "2099-01-01T00:00:00Z", "max_request_bytes": 5242880})
}

impl Fake {
    pub fn start() -> Fake {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let state = Arc::new(Mutex::new(State::default()));
        let s2 = Arc::clone(&state);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let st = Arc::clone(&s2);
                std::thread::spawn(move || {
                    if let Some(req) = read_req(&mut stream) {
                        Fake::handle(&st, &mut stream, req);
                    }
                });
            }
        });
        Fake { port, state }
    }

    pub fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn handle(state: &Arc<Mutex<State>>, stream: &mut TcpStream, req: Req) {
        let mut st = state.lock().unwrap();
        let ticket_ok = req.headers.get("x-allternit-wake-ticket").map(|t| t == TICKET).unwrap_or(false);
        if ticket_ok {
            st.ticketed_requests += 1;
        }
        let consent = json!({"wake": {"setting": "ask", "consent_path": format!("/api/v1/computer-files/{COMPUTER}/wake-consent")},
                             "computer_state": "asleep"});
        let p = req.path.clone();
        if p == format!("/api/v1/computer-files/{COMPUTER}/wake-consent") && req.method == "POST" {
            st.consent_calls += 1;
            st.asleep = false;
            let body = json!({"ticket": TICKET, "expires_at": "2099-01-01T00:00:00Z", "computer_state": "awake", "estimated_wake_s": 0});
            return respond(stream, 200, "application/json", &[], body.to_string().as_bytes());
        }
        let Some(rest) = p.strip_prefix("/api/v1/files/v2") else {
            return respond(stream, 404, "text/plain", &[], b"no");
        };
        let seg: Vec<&str> = rest.trim_start_matches('/').split('/').collect();
        let content_route = matches!((req.method.as_str(), seg.as_slice()), ("GET", ["items", _, "content"]))
            || seg.first() == Some(&"uploads")
            || matches!(seg.as_slice(), ["mkdir"] | ["move"] | ["delete"]);
        if content_route && st.asleep && !ticket_ok {
            st.content_refused += 1;
            let (s, v) = err("wake_consent_required", 409, consent);
            return respond(stream, s, "application/json", &[], v.to_string().as_bytes());
        }
        let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
        let (status, out): (u16, Value) = match (req.method.as_str(), seg.as_slice()) {
            ("GET", ["roots"]) => {
                let roots: Vec<Value> = st
                    .roots
                    .iter()
                    .map(|r| {
                        json!({"id": r.id, "kind": r.kind, "name": r.name, "root_item_id": r.item, "cursor": st.cursor(&r.id),
                               "provider_ready": true,
                               "policy": {"read": true, "write": true, "shareable": true, "agent_process": false,
                                          "protected": r.protected, "offline": false, "replica": false,
                                          "versions": {"keep_count": 10, "keep_days": 30}, "trash_days": 30}})
                    })
                    .collect();
                (200, json!({"computer_id": COMPUTER, "roots": roots, "freshness": st.freshness()}))
            }
            ("GET", ["roots", root, "list"]) => {
                let parent = req.query.get("parent_id").cloned().unwrap_or_default();
                match st.items.get(&parent) {
                    None => err("not_found", 404, json!({})),
                    Some(pi) => {
                        let items: Vec<Value> = st.items.values().filter(|i| !i.trashed && i.parent.as_deref() == Some(parent.as_str())).map(State::item_json).collect();
                        (200, json!({"root_id": root, "parent": State::item_json(pi), "items": items, "cursor": st.cursor(root), "freshness": st.freshness()}))
                    }
                }
            }
            ("GET", ["items", id]) => match st.items.get(*id) {
                Some(i) => (200, json!({"item": State::item_json(i), "freshness": st.freshness()})),
                None => err("not_found", 404, json!({})),
            },
            ("GET", ["items", id, "content"]) => {
                let Some(it) = st.items.get(*id) else {
                    let (s, v) = err("not_found", 404, json!({}));
                    return respond(stream, s, "application/json", &[], v.to_string().as_bytes());
                };
                let data = it.data.clone();
                let etag = format!("\"{}\"", State::rev(it));
                let (a, b) = req
                    .headers
                    .get("range")
                    .and_then(|r| r.strip_prefix("bytes="))
                    .and_then(|r| r.split_once('-'))
                    .map(|(a, b)| (a.parse::<u64>().unwrap(), b.parse::<u64>().unwrap()))
                    .unwrap_or((0, data.len() as u64 - 1));
                let b = b.min(data.len() as u64 - 1);
                st.ranges.push((a, b - a + 1));
                let slice = data[a as usize..=b as usize].to_vec();
                drop(st);
                return respond(stream, 206, "application/octet-stream", &[("ETag", etag)], &slice);
            }
            ("GET", ["roots", root, "changes"]) => {
                let after: usize = req.query.get("cursor").and_then(|c| c.strip_prefix("c1.")).and_then(|n| n.parse().ok()).unwrap_or(0);
                let changes: Vec<Value> = st.changes.iter().skip(after).filter(|(r, _)| r == root).map(|(_, c)| c.clone()).collect();
                (200, json!({"root_id": root, "changes": changes, "cursor": format!("c1.{}", st.changes.len()),
                             "has_more": false, "reset_required": false, "freshness": st.freshness()}))
            }
            ("POST", ["uploads"]) => {
                let sid = st.id("up");
                let refs = body["chunks"].as_array().cloned().unwrap_or_default();
                let out = session_status(&sid, &st, &body, &refs);
                st.sessions.insert(sid, (body.clone(), refs));
                (200, out)
            }
            ("POST", ["uploads", sid, "chunks"]) => {
                let more = body["chunks"].as_array().cloned().unwrap_or_default();
                let (req0, refs) = st.sessions.get_mut(*sid).map(|(r, refs)| {
                    refs.extend(more);
                    (r.clone(), refs.clone())
                }).unwrap();
                (200, session_status(sid, &st, &req0, &refs))
            }
            ("GET", ["uploads", sid]) => {
                let (r, refs) = st.sessions[*sid].clone();
                (200, session_status(sid, &st, &r, &refs))
            }
            ("PUT", ["uploads", _sid, "chunks", h]) => {
                assert_eq!(hash(&req.body), *h, "chunk bytes match their hash");
                assert!(req.body.len() <= 4 * 1024 * 1024, "chunk over 4 MiB");
                st.chunks.insert(h.to_string(), req.body.clone());
                (200, json!({"hash": h, "stored": true, "remaining": 0}))
            }
            ("POST", ["uploads", sid, "commit"]) => {
                let (r, refs) = st.sessions.remove(*sid).unwrap();
                let mut data = Vec::new();
                for c in &refs {
                    data.extend_from_slice(&st.chunks[c["hash"].as_str().unwrap()]);
                }
                assert_eq!(hash(&data), r["content_hash"].as_str().unwrap(), "whole-file hash");
                let parent = r["parent_id"].as_str().unwrap().to_string();
                let root = r["root_id"].as_str().unwrap().to_string();
                let name = r["name"].as_str().unwrap().to_string();
                match r["item_id"].as_str() {
                    Some(item_id) => {
                        let current = State::rev(&st.items[item_id]);
                        if r["base_rev"].as_str() == Some(current.as_str()) {
                            st.modify(item_id, &data);
                            let cur = st.cursor(&root);
                            (200, json!({"item": State::item_json(&st.items[item_id]), "cursor": cur}))
                        } else {
                            // keep_both: the write becomes a conflict copy beside the original.
                            let (stem, ext) = name.rsplit_once('.').map(|(s, e)| (s.to_string(), format!(".{e}"))).unwrap_or((name.clone(), String::new()));
                            let device = r["device_name"].as_str().unwrap_or("Device");
                            let cname = format!("{stem} (conflict {device} 2026-11-15 1042){ext}");
                            let cid = st.add(&root, Some(&parent), &cname, false, &data);
                            let orig = State::item_json(&st.items[item_id]);
                            let cur = st.cursor(&root);
                            (200, json!({"item": orig, "cursor": cur, "conflict": {
                                "kind": "edit_edit", "original_item_id": item_id, "base_rev": r["base_rev"], "current_rev": current,
                                "conflict_item": State::item_json(&st.items[&cid]), "conflict_name": cname}}))
                        }
                    }
                    None => {
                        let n = st.free_name(&parent, &name);
                        let id = st.add(&root, Some(&parent), &n, false, &data);
                        let cur = st.cursor(&root);
                        (200, json!({"item": State::item_json(&st.items[&id]), "cursor": cur}))
                    }
                }
            }
            ("POST", ["mkdir"]) => {
                let parent = body["parent_id"].as_str().unwrap().to_string();
                let root = st.items[&parent].root.clone();
                let n = st.free_name(&parent, body["name"].as_str().unwrap());
                let id = st.add(&root, Some(&parent), &n, true, b"");
                let cur = st.cursor(&root);
                (200, json!({"item": State::item_json(&st.items[&id]), "cursor": cur}))
            }
            ("POST", ["move"]) => {
                let id = body["item_id"].as_str().unwrap().to_string();
                if body["base_rev"].as_str() != Some(State::rev(&st.items[&id]).as_str()) {
                    err("rev_mismatch", 409, json!({}))
                } else {
                    let it = st.items.get_mut(&id).unwrap();
                    if let Some(n) = body["new_name"].as_str() {
                        it.name = n.into();
                    }
                    if let Some(p) = body["new_parent_id"].as_str() {
                        it.parent = Some(p.into());
                    }
                    it.mv += 1;
                    st.record("moved", &id);
                    let root = st.items[&id].root.clone();
                    let cur = st.cursor(&root);
                    (200, json!({"item": State::item_json(&st.items[&id]), "cursor": cur}))
                }
            }
            ("POST", ["delete"]) => {
                let id = body["item_id"].as_str().unwrap().to_string();
                match st.items.get(&id) {
                    None => err("not_found", 404, json!({})),
                    Some(_) => {
                        st.trash(&id);
                        let root = st.items[&id].root.clone();
                        let cur = st.cursor(&root);
                        (200, json!({"cursor": cur}))
                    }
                }
            }
            ("POST", ["pin"]) => (200, json!({"item_id": body["item_id"], "pinned_on": if body["pinned"] == json!(true) { json!(["dev_test"]) } else { json!([]) }})),
            _ => err("not_found", 404, json!({})),
        };
        drop(st);
        respond(stream, status, "application/json", &[], out.to_string().as_bytes());
    }

    /// The test tree: Allternit (hello.txt, big.bin, docs/note.md), Code (readme.md),
    /// and a protected Memory Drive that must not show in Explorer.
    pub fn seed(&self, big: &[u8]) -> BTreeMap<&'static str, String> {
        let mut st = self.state.lock().unwrap();
        let mut ids = BTreeMap::new();
        for (rid, kind, name, protected) in
            [("rt_home", "allternit_home", "Allternit", false), ("rt_code", "code", "Code", false), ("rt_mem", "memory_drive", "Memory Drive", true)]
        {
            let item = st.add(rid, None, name, true, b"");
            st.roots.push(Root { id: rid.into(), kind: kind.into(), name: name.into(), item: item.clone(), protected });
            ids.insert(rid, item);
        }
        let (home, code, mem) = (ids["rt_home"].clone(), ids["rt_code"].clone(), ids["rt_mem"].clone());
        ids.insert("hello", st.add("rt_home", Some(&home), "hello.txt", false, b"hello from the allternit fake runtime\n"));
        ids.insert("big", st.add("rt_home", Some(&home), "big.bin", false, big));
        let docs = st.add("rt_home", Some(&home), "docs", true, b"");
        ids.insert("note", st.add("rt_home", Some(&docs), "note.md", false, b"# note\n"));
        ids.insert("docs", docs);
        ids.insert("readme", st.add("rt_code", Some(&code), "readme.md", false, b"# code\n"));
        st.add("rt_mem", Some(&mem), "secret.txt", false, b"never listed\n");
        // The tree is the starting state, not a change stream.
        st.changes.clear();
        ids
    }
}
