//! A pane backend for in-process engine tests: each "pane" is a real child
//! process (the same `/bin/sh <runner>` the pane engine would run), so drive,
//! send and the registry are exercised end to end without a pane engine
//! daemon. Tests that run the `allternit-factory` binary use the real pane
//! engine instead.

#![allow(dead_code)]

use std::collections::HashMap;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

use allternit_factory_engine::backend::{self, LivePane, PaneBackend, PaneSend, PaneSpawn};

#[derive(Default)]
pub struct FakePanes {
    children: Mutex<HashMap<String, (String, Child, String)>>,
    pub sent: Mutex<Vec<(String, String)>>,
    /// When true, sends to a live pane are "busy" and go to the queue.
    pub busy: Mutex<bool>,
    next: Mutex<u64>,
}

impl FakePanes {
    /// Install a fresh fake as the process's pane backend.
    pub fn install() -> Arc<FakePanes> {
        let fake = Arc::new(FakePanes::default());
        backend::install(fake.clone());
        fake
    }
}

impl PaneBackend for FakePanes {
    fn spawn(&self, req: &PaneSpawn) -> anyhow::Result<LivePane> {
        let mut children = self.children.lock().unwrap();
        if let Some((_, child, _)) = children.get_mut(&req.session) {
            if child.try_wait()?.is_none() {
                anyhow::bail!("session {} already exists", req.session);
            }
        }
        let log = match &req.transcript {
            Some(p) => Stdio::from(std::fs::File::create(p)?),
            None => Stdio::null(),
        };
        let child = Command::new(&req.argv[0])
            .args(&req.argv[1..])
            .current_dir(&req.cwd)
            .envs(&req.env)
            .stdin(Stdio::null())
            .stdout(log)
            .stderr(Stdio::null())
            .spawn()?;
        let mut next = self.next.lock().unwrap();
        *next += 1;
        let pane_id = format!("p{}", *next);
        let cwd = req.cwd.to_string_lossy().to_string();
        children.insert(req.session.clone(), (pane_id.clone(), child, cwd.clone()));
        Ok(LivePane { session: req.session.clone(), pane_id, cwd: Some(cwd), agent_status: None })
    }

    fn list(&self) -> anyhow::Result<Vec<LivePane>> {
        let mut children = self.children.lock().unwrap();
        let mut out = Vec::new();
        for (session, (pane_id, child, cwd)) in children.iter_mut() {
            if child.try_wait()?.is_none() {
                out.push(LivePane {
                    session: session.clone(),
                    pane_id: pane_id.clone(),
                    cwd: Some(cwd.clone()),
                    agent_status: Some("idle".into()),
                });
            }
        }
        Ok(out)
    }

    fn send(&self, _root: &Path, session: &str, text: &str, _sender: &str, queue_only: bool) -> anyhow::Result<PaneSend> {
        let live = self.find(session)?.is_some();
        if live && !queue_only && !*self.busy.lock().unwrap() {
            self.sent.lock().unwrap().push((session.to_string(), text.to_string()));
            return Ok(PaneSend::Verified);
        }
        let reason = if queue_only { "queued on request" } else if live { "the pane is busy" } else { "the pane is not running" };
        Ok(PaneSend::Queued { message_id: "1".into(), depth: 1, reason: reason.into() })
    }

    fn capture(&self, session: &str, _lines: u32) -> anyhow::Result<String> {
        Ok(format!("screen of {session}"))
    }

    fn kill(&self, session: &str) -> anyhow::Result<()> {
        if let Some((_, child, _)) = self.children.lock().unwrap().get_mut(session) {
            let _ = child.kill();
            let _ = child.wait();
        }
        Ok(())
    }
}
