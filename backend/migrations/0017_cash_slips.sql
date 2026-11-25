-- A cash sale leaves a slip too, in the same permanent table as the PromptPay ones.
-- `cash_received_satang` is what the customer handed over (the change follows from it).
ALTER TABLE payment_slips ADD COLUMN cash_received_satang INTEGER;

-- Cash sales already made become slips as well.
INSERT INTO payment_slips
    (shop_id, order_id, queue_no, queue_date, amount_satang, method, verified,
     items, paid_at, cash_received_satang)
SELECT p.shop_id, o.id, o.queue_no, o.queue_date, p.amount_satang, 'cash', 0,
       COALESCE((SELECT json_group_array(json_object('name', i.name, 'qty', i.qty, 'price', i.unit_price_satang))
                 FROM order_items i WHERE i.order_id = o.id), '[]'),
       COALESCE(o.paid_at, p.created_at), o.cash_received_satang
FROM payments p JOIN orders o ON o.id = p.order_id
WHERE p.method = 'cash'
  AND NOT EXISTS (SELECT 1 FROM payment_slips s WHERE s.order_id = o.id AND s.method = 'cash');
