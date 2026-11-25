CREATE TABLE shops (
    id            INTEGER PRIMARY KEY,
    slug          TEXT NOT NULL UNIQUE,
    name          TEXT NOT NULL,
    promptpay_id  TEXT NOT NULL DEFAULT '',
    created_at    TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE users (
    id            INTEGER PRIMARY KEY,
    shop_id       INTEGER NOT NULL REFERENCES shops(id),
    username      TEXT NOT NULL,
    password_hash TEXT NOT NULL,
    role          TEXT NOT NULL DEFAULT 'admin',
    UNIQUE (shop_id, username)
);

CREATE TABLE products (
    id            INTEGER PRIMARY KEY,
    shop_id       INTEGER NOT NULL REFERENCES shops(id),
    name          TEXT NOT NULL,
    category      TEXT NOT NULL DEFAULT '',
    price_satang  INTEGER NOT NULL CHECK (price_satang >= 0),
    active        INTEGER NOT NULL DEFAULT 1,
    sort          INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX idx_products_shop ON products(shop_id, active, sort);

CREATE TABLE orders (
    id            INTEGER PRIMARY KEY,
    shop_id       INTEGER NOT NULL REFERENCES shops(id),
    token         TEXT NOT NULL UNIQUE,
    queue_date    TEXT NOT NULL,
    queue_no      INTEGER,
    status        TEXT NOT NULL CHECK (status IN ('pending_payment','paid','called','done','cancelled')),
    method        TEXT NOT NULL CHECK (method IN ('cash','promptpay')),
    source        TEXT NOT NULL CHECK (source IN ('pos','self')),
    total_satang  INTEGER NOT NULL CHECK (total_satang >= 0),
    pay_ref       TEXT,
    pay_state     TEXT,
    qr_payload    TEXT,
    created_at    TEXT NOT NULL DEFAULT (datetime('now')),
    paid_at       TEXT,
    called_at     TEXT,
    done_at       TEXT
);
CREATE UNIQUE INDEX idx_orders_queue ON orders(shop_id, queue_date, queue_no) WHERE queue_no IS NOT NULL;
CREATE INDEX idx_orders_shop_status ON orders(shop_id, status);

CREATE TABLE order_items (
    id                 INTEGER PRIMARY KEY,
    order_id           INTEGER NOT NULL REFERENCES orders(id) ON DELETE CASCADE,
    product_id         INTEGER NOT NULL,
    name               TEXT NOT NULL,
    unit_price_satang  INTEGER NOT NULL,
    qty                INTEGER NOT NULL CHECK (qty > 0)
);
CREATE INDEX idx_items_order ON order_items(order_id);

CREATE TABLE payments (
    id               INTEGER PRIMARY KEY,
    shop_id          INTEGER NOT NULL REFERENCES shops(id),
    order_id         INTEGER NOT NULL REFERENCES orders(id),
    amount_satang    INTEGER NOT NULL,
    method           TEXT NOT NULL,
    external_id      TEXT UNIQUE,
    payer_name       TEXT,
    bank             TEXT,
    created_at       TEXT NOT NULL DEFAULT (datetime('now'))
);
