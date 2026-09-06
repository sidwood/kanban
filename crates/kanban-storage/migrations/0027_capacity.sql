-- 0027 capacity
--
-- Global capacity defaults and stricter per-Project caps (CONTEXT.md,
-- DR-EP-06, DR-EP-07). The global row carries the maximum active
-- runs one harness, model family, or usage pool may hold across
-- every Project; a Project row carries the stricter ceilings it
-- imposes plus its maximum active Lane count. A NULL cap constrains
-- nothing, and the schema refuses zero limits and non-positive
-- versions. Unlike the Herdr settings, no row is backfilled or
-- seeded per Project: absence is the honest record of a Project
-- that imposes nothing, and the read path answers unset caps at
-- version 1 while the first update inserts.

CREATE TABLE capacity_global_defaults (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    max_active_per_harness INTEGER NOT NULL DEFAULT 2 CHECK (max_active_per_harness > 0),
    max_active_per_model INTEGER NOT NULL DEFAULT 2 CHECK (max_active_per_model > 0),
    max_active_per_usage_pool INTEGER NOT NULL DEFAULT 4 CHECK (max_active_per_usage_pool > 0),
    version INTEGER NOT NULL DEFAULT 1 CHECK (version > 0)
);

INSERT INTO capacity_global_defaults (
    id,
    max_active_per_harness,
    max_active_per_model,
    max_active_per_usage_pool,
    version
) VALUES (1, 2, 2, 4, 1);

CREATE TABLE capacity_project_caps (
    project_id INTEGER PRIMARY KEY REFERENCES projects(id),
    max_active_per_harness INTEGER CHECK (max_active_per_harness > 0),
    max_active_per_model INTEGER CHECK (max_active_per_model > 0),
    max_active_per_usage_pool INTEGER CHECK (max_active_per_usage_pool > 0),
    max_active_lanes INTEGER CHECK (max_active_lanes > 0),
    version INTEGER NOT NULL DEFAULT 1 CHECK (version > 0)
);
