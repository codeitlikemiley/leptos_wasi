CREATE TABLE counters (
    counter_id TEXT PRIMARY KEY,
    value INTEGER NOT NULL CHECK (value >= 0)
);

CREATE TABLE counter_operations (
    counter_id TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    result_value INTEGER,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (counter_id, operation_id),
    FOREIGN KEY (counter_id) REFERENCES counters(counter_id) DEFERRABLE INITIALLY DEFERRED
);
