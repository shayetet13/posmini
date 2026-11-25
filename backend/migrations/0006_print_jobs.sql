-- Every sheet to print is a row here, taken strictly in id order by one worker, so tickets come
-- out in the same order the queue numbers were handed out, and nothing is lost if the printer
-- is off or the server restarts.
CREATE TABLE print_jobs (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    shop_id     INTEGER NOT NULL REFERENCES shops(id),
    order_id    INTEGER NOT NULL REFERENCES orders(id),
    kind        TEXT NOT NULL CHECK (kind IN ('ticket', 'slip', 'receipt')),
    status      TEXT NOT NULL DEFAULT 'queued' CHECK (status IN ('queued', 'done', 'failed', 'skipped')),
    attempts    INTEGER NOT NULL DEFAULT 0,
    error       TEXT,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    printed_at  TEXT
);
CREATE INDEX idx_print_jobs_queue ON print_jobs(shop_id, status, id);
