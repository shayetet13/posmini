-- The customer's LINE user id, kept only when they asked to be told on LINE that their queue is called.
ALTER TABLE orders ADD COLUMN line_user_id TEXT;
