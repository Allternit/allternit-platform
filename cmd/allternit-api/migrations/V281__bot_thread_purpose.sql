-- Threads with a purpose: a one-line purpose, an icon key and a colour key.
-- `purpose` keeps the thread's work on topic (it joins the thread's system
-- context); `icon` and `color` are keys from the fixed sets in
-- thread_routes.rs (THREAD_ICONS / THREAD_COLORS), never raw values.
ALTER TABLE bot_threads ADD COLUMN purpose TEXT;
ALTER TABLE bot_threads ADD COLUMN icon TEXT;
ALTER TABLE bot_threads ADD COLUMN color TEXT;
