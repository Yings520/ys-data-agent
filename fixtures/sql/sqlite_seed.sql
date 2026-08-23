CREATE TABLE mart_orders (
    order_id INTEGER PRIMARY KEY,
    paid_amount REAL NOT NULL,
    paid_at TEXT NOT NULL,
    customer_email TEXT NOT NULL,
    country TEXT NOT NULL,
    channel TEXT NOT NULL
);

CREATE TABLE raw_customers (
    customer_id INTEGER PRIMARY KEY,
    customer_email TEXT NOT NULL
);

INSERT INTO mart_orders (
    order_id,
    paid_amount,
    paid_at,
    customer_email,
    country,
    channel
) VALUES
    (1, 120.50, '2026-08-12T10:00:00Z', 'alice@example.com', 'SG', 'web'),
    (2, 80.00,  '2026-08-13T11:00:00Z', 'bob@example.com',   'SG', 'store'),
    (3, 50.25,  '2026-08-14T12:00:00Z', 'cara@example.com', 'MY', 'web');

INSERT INTO raw_customers (customer_id, customer_email) VALUES
    (1, 'alice@example.com');
