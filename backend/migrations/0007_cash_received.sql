-- Cash handed over by the customer, so the bill can show the amount received and the change.
ALTER TABLE orders ADD COLUMN cash_received_satang INTEGER;
