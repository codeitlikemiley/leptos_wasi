CREATE TABLE counters (
    counter_id TEXT PRIMARY KEY,
    value BIGINT NOT NULL CHECK (value >= 0)
);

CREATE TABLE counter_operations (
    counter_id TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    result_value BIGINT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (counter_id, operation_id),
    FOREIGN KEY (counter_id) REFERENCES counters(counter_id) DEFERRABLE INITIALLY DEFERRED
);
