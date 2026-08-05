DROP TABLE IF EXISTS orders;
DROP TABLE IF EXISTS customers;

CREATE TABLE customers (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    region TEXT NOT NULL
);

CREATE TABLE orders (
    id INTEGER PRIMARY KEY,
    customer_id INTEGER NOT NULL,
    ordered_at TEXT NOT NULL,
    amount REAL NOT NULL,
    FOREIGN KEY (customer_id) REFERENCES customers(id)
);

INSERT INTO customers (id, name, region) VALUES
    (1, 'Acme Singapore', 'APAC'),
    (2, 'Northwind GmbH', 'EMEA'),
    (3, 'Contoso US', 'AMER');

INSERT INTO orders (id, customer_id, ordered_at, amount) VALUES
    (1, 1, '2026-07-01', 1200.00),
    (2, 1, '2026-07-12', 850.00),
    (3, 2, '2026-07-03', 640.00),
    (4, 2, '2026-07-20', 910.00),
    (5, 3, '2026-07-08', 1500.00);
