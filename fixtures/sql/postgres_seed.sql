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
