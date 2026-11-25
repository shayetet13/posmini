-- Closing the day: a frozen copy of the takings of one day, with the cash that was counted in the
-- drawer and the difference. Append-only; a day closed twice has two rows and the latest wins.
CREATE TABLE day_closes (
    id                  INTEGER PRIMARY KEY,
    shop_id             INTEGER NOT NULL REFERENCES shops(id),
    day                 TEXT NOT NULL,
    closed_at           TEXT NOT NULL DEFAULT (datetime('now')),
    closed_by           TEXT NOT NULL,
    orders              INTEGER NOT NULL,
    revenue_satang      INTEGER NOT NULL,
    cash_satang         INTEGER NOT NULL,
    qr_satang           INTEGER NOT NULL,
    manual_confirms     INTEGER NOT NULL DEFAULT 0,
    cancelled_paid      INTEGER NOT NULL DEFAULT 0,
    cash_counted_satang INTEGER,
    difference_satang   INTEGER,
    note                TEXT NOT NULL DEFAULT ''
);
CREATE INDEX idx_day_closes ON day_closes(shop_id, day, id);
