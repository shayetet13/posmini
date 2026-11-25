-- A sale made while the till had no connection is sent later. `client_id` is made by the till so that
-- sending the same sale twice (a retry after a dropped answer) never makes two orders.
ALTER TABLE orders ADD COLUMN client_id TEXT;
ALTER TABLE orders ADD COLUMN offline_at TEXT;
CREATE UNIQUE INDEX orders_client_id ON orders (shop_id, client_id) WHERE client_id IS NOT NULL;
