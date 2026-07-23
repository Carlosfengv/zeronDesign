CREATE TABLE IF NOT EXISTS provider_connections (
    id TEXT PRIMARY KEY,
    version BIGINT NOT NULL,
    enabled BOOLEAN NOT NULL,
    connection_json TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS gateway_configuration_state (
    singleton_id INTEGER PRIMARY KEY CHECK (singleton_id = 1),
    configuration_version BIGINT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

INSERT INTO gateway_configuration_state (singleton_id, configuration_version)
VALUES (1, 0)
ON CONFLICT (singleton_id) DO NOTHING;
