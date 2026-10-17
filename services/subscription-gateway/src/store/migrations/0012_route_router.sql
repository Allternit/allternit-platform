-- Route router (spec §3/§4, task P0): per-(vendor, route, app_version)
-- capability-contract results, per-vendor route bindings + Settings
-- overrides, per-app-version launch probes (§2 debug-mode rule), and the
-- append-only choice log. No secrets here: conformance evidence is check
-- names/reasons only.

-- §3 conformance results, keyed by app version: an app update leaves the old
-- rows in place but stops matching them, so a new app version is ineligible
-- until the suite re-runs against it.
CREATE TABLE route_conformance (
  vendor TEXT NOT NULL,
  route TEXT NOT NULL,
  app_version TEXT NOT NULL,
  item TEXT NOT NULL,
  passed INTEGER NOT NULL,
  reason TEXT,
  evidence TEXT NOT NULL DEFAULT '[]',
  checked_at TEXT NOT NULL,
  PRIMARY KEY (vendor, route, app_version, item)
);
CREATE INDEX route_conformance_vendor ON route_conformance (vendor, route, checked_at);

-- §4.2 the chosen route, written into the vendor's binding. One row per
-- vendor: two routes never run the same turn.
CREATE TABLE route_bindings (
  vendor TEXT PRIMARY KEY,
  bound_route TEXT,
  reason TEXT,
  updated_at TEXT NOT NULL
);

-- §4.6 the Settings pin. route NULL deletes the row (pin cleared).
CREATE TABLE route_overrides (
  vendor TEXT PRIMARY KEY,
  route TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

-- §2 debug-mode rule: a launch-time probe records which desktop lane applies
-- per app version (cdp only when the shipped app accepts its debug flag
-- unmodified; otherwise the app uses desktop-app-ax). Missing row for the
-- current app version → the desktop route is not eligible (never assumed).
CREATE TABLE route_probes (
  vendor TEXT NOT NULL,
  route TEXT NOT NULL,
  app_version TEXT NOT NULL,
  applies INTEGER NOT NULL,
  detail TEXT,
  probed_at TEXT NOT NULL,
  PRIMARY KEY (vendor, route, app_version)
);
