-- A coordinator plan is mirrored into the canonical rails DAG (spec P5.1/P5.6):
-- each task thread records the DAG and node that stand for it, so status
-- moves on the graph and the goal loop / WIH / rails views read one graph.
ALTER TABLE bot_threads ADD COLUMN dag_id TEXT;
ALTER TABLE bot_threads ADD COLUMN dag_node_id TEXT;
