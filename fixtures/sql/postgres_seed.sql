CREATE TABLE public.orders (
    order_id BIGINT PRIMARY KEY,
    paid_amount NUMERIC(18, 2) NOT NULL,
    paid_at TIMESTAMPTZ NOT NULL,
    customer_email TEXT NOT NULL
);

INSERT INTO public.orders (
    order_id,
    paid_amount,
    paid_at,
    customer_email
) VALUES
    (1, 120.50, '2026-08-12T10:00:00Z', 'alice@example.com'),
    (2, 80.00,  '2026-08-13T11:00:00Z', 'bob@example.com'),
    (3, 50.25,  '2026-08-14T12:00:00Z', 'cara@example.com');

-- Managed-connector fixture: login can read the exact target but cannot own or mutate it.
REVOKE TEMPORARY ON DATABASE ysda_test FROM PUBLIC;
REVOKE CREATE ON SCHEMA public FROM PUBLIC;
CREATE ROLE ysda_reader LOGIN PASSWORD 'ysda-reader-test' NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS;
GRANT CONNECT ON DATABASE ysda_test TO ysda_reader;
GRANT USAGE ON SCHEMA public TO ysda_reader;
GRANT SELECT ON TABLE public.orders TO ysda_reader;
ALTER ROLE ysda_reader SET default_transaction_read_only = on;
