-- The menu becomes a tree: a "group" row (is_group = 1, no price) holds items and other groups
-- through parent_id; a row with no parent sits at the top. Every row has its own card colour.
ALTER TABLE products ADD COLUMN parent_id INTEGER REFERENCES products(id);
ALTER TABLE products ADD COLUMN is_group  INTEGER NOT NULL DEFAULT 0;
ALTER TABLE products ADD COLUMN color     TEXT;
CREATE INDEX idx_products_parent ON products(shop_id, parent_id, active, sort);
