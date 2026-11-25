-- A permanent record of every PromptPay payment (the "slip"): who paid, which bank, how much,
-- for which order. It is a snapshot with no link to `orders`, so it survives "start over" and any
-- later clean-up of orders, and the dashboard can show it for any past day.
CREATE TABLE payment_slips (
    id            INTEGER PRIMARY KEY,
    shop_id       INTEGER NOT NULL REFERENCES shops(id),
    order_id      INTEGER NOT NULL,
    queue_no      INTEGER,
    queue_date    TEXT,
    amount_satang INTEGER NOT NULL,
    method        TEXT NOT NULL,
    verified      INTEGER NOT NULL DEFAULT 0,
    payer_name    TEXT,
    bank          TEXT,
    external_id   TEXT,
    items         TEXT NOT NULL DEFAULT '[]',
    paid_at       TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE UNIQUE INDEX idx_slips_external ON payment_slips(external_id) WHERE external_id IS NOT NULL;
CREATE INDEX idx_slips_time ON payment_slips(shop_id, paid_at);

-- Payments already made (for example everything from 24/11/2569) become slips too.
INSERT INTO payment_slips
    (shop_id, order_id, queue_no, queue_date, amount_satang, method, verified,
     payer_name, bank, external_id, items, paid_at)
SELECT p.shop_id, o.id, o.queue_no, o.queue_date, p.amount_satang, p.method,
       CASE WHEN p.external_id IS NOT NULL THEN 1 ELSE 0 END,
       p.payer_name, p.bank, p.external_id,
       COALESCE((SELECT json_group_array(json_object('name', i.name, 'qty', i.qty, 'price', i.unit_price_satang))
                 FROM order_items i WHERE i.order_id = o.id), '[]'),
       COALESCE(o.paid_at, p.created_at)
FROM payments p JOIN orders o ON o.id = p.order_id
WHERE p.method = 'promptpay';
