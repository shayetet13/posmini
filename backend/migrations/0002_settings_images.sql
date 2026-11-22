CREATE TABLE settings (
    shop_id  INTEGER NOT NULL REFERENCES shops(id),
    key      TEXT NOT NULL,
    value    TEXT NOT NULL,
    PRIMARY KEY (shop_id, key)
);

ALTER TABLE products ADD COLUMN image_mime TEXT;
ALTER TABLE products ADD COLUMN image_data BLOB;
ALTER TABLE products ADD COLUMN image_v    INTEGER NOT NULL DEFAULT 0;
