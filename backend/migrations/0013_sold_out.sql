-- "Sold out" for a menu item: hidden from ordering on the till and on the customer's menu until it is
-- switched back, without deleting the item.
ALTER TABLE products ADD COLUMN sold_out INTEGER NOT NULL DEFAULT 0;
