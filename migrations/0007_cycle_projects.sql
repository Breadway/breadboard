CREATE TABLE cycle_projects (
    cycle_id INTEGER NOT NULL REFERENCES cycles(id) ON DELETE CASCADE,
    project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    PRIMARY KEY (cycle_id, project_id)
);

CREATE INDEX idx_cycle_projects_project ON cycle_projects(project_id);
