-- Staff accounts: an account can be switched off without deleting it, and shows when it was made.
ALTER TABLE users ADD COLUMN active     INTEGER NOT NULL DEFAULT 1;
ALTER TABLE users ADD COLUMN created_at TEXT;

-- Who did what that touches money or the record: one row per action, never edited.
CREATE TABLE audit_log (
    id        INTEGER PRIMARY KEY,
    shop_id   INTEGER NOT NULL REFERENCES shops(id),
    at        TEXT NOT NULL DEFAULT (datetime('now')),
    actor     TEXT NOT NULL,
    role      TEXT NOT NULL DEFAULT '',
    action    TEXT NOT NULL,
    order_id  INTEGER,
    detail    TEXT NOT NULL DEFAULT ''
);
CREATE INDEX idx_audit_time ON audit_log(shop_id, at);
