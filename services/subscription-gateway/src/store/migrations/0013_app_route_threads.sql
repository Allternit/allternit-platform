-- Model-picker chats on a phone or desktop-app route (src/routes/app-route-tasks.ts):
-- which app conversation each fabric thread continues in, so a gateway
-- restart doesn't start a new chat in the app.
CREATE TABLE app_route_threads (
  thread_id TEXT PRIMARY KEY,
  adapter_id TEXT NOT NULL,
  context_id TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
