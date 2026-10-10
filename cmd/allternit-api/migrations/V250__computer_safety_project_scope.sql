-- Computer-use safety settings: project scope (task K4).
-- computer_safety_settings.scope (V246) gains 'project:<id>', where <id> is
-- a cowork_projects row owned by the settings' owner — the API verifies
-- ownership at write time (GET/PUT /projects/:id/safety), so the table stays
-- a plain owner/scope key-value store with no foreign key. The effective
-- chain for a toolset call is computer -> project -> owner default, the most
-- specific set field winning per computer_safety::over; a call's project is
-- its run's session project, never a project_id taken blindly from a model.
CREATE INDEX IF NOT EXISTS idx_computer_safety_settings_scope ON computer_safety_settings(scope);
