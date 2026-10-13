-- Trained flywheel heads (agency_api::decisions::training): one row per
-- (head, kind, version). `head.embed` keeps its scorer weights (f32 LE) and
-- the embedding model id + dimension they were trained on (the head refuses
-- vectors from any other model); `head.rules` keeps its decision list as
-- readable JSON. Metrics, the dataset hash and the example count say what a
-- version was trained on and how it scored on its validation split.
CREATE TABLE IF NOT EXISTS decision_head_models (
    id TEXT PRIMARY KEY,
    head TEXT NOT NULL,
    kind TEXT NOT NULL,
    version INTEGER NOT NULL,
    embed_model TEXT,
    embed_dim INTEGER,
    weights BLOB,
    rules_json TEXT,
    metrics_json TEXT NOT NULL,
    dataset_hash TEXT NOT NULL,
    examples INTEGER NOT NULL,
    actor TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE (head, kind, version)
);

-- Training jobs: POST .../heads/train|compile/:kind queue one, GET
-- .../heads/jobs/:id follows it. Progress counts texts embedded of the total.
-- At most one queued or running job per (head, kind).
CREATE TABLE IF NOT EXISTS decision_head_jobs (
    id TEXT PRIMARY KEY,
    head TEXT NOT NULL,
    kind TEXT NOT NULL,
    status TEXT NOT NULL,
    progress_done INTEGER NOT NULL DEFAULT 0,
    progress_total INTEGER NOT NULL DEFAULT 0,
    result_json TEXT,
    error TEXT,
    actor TEXT NOT NULL,
    created_at TEXT NOT NULL,
    started_at TEXT,
    finished_at TEXT,
    updated_at TEXT NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_decision_head_jobs_active
    ON decision_head_jobs(head, kind) WHERE status IN ('queued', 'running');
