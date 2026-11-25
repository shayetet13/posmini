-- Notes on lines and orders, a discount on the whole bill, and why an order was cancelled.
ALTER TABLE order_items ADD COLUMN note TEXT NOT NULL DEFAULT '';
ALTER TABLE orders ADD COLUMN note TEXT NOT NULL DEFAULT '';
ALTER TABLE orders ADD COLUMN discount_satang INTEGER NOT NULL DEFAULT 0;
ALTER TABLE orders ADD COLUMN discount_reason TEXT NOT NULL DEFAULT '';
ALTER TABLE orders ADD COLUMN cancel_reason TEXT NOT NULL DEFAULT '';
