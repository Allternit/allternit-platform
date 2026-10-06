# Runner Mutations & Autopipeline Contracts

- `runner_mutations.json` lists the ledger mutation IDs that the Rails runner produces when driving the Ralph loop (loop iterations, spawn requests, vault/archive events, closeout events, etc.).
- Use this reference when extending the runner to ensure new events are appended consistently and to regenerate derived views like `.allternit/meta/rails_cursor.json`.
- Run `cargo test -p allternit-commrails` after touching the runner to prove the autopipeline sequence still passes.
