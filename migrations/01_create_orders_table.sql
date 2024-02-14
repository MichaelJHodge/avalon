CREATE TABLE orders (
    id INTEGER PRIMARY KEY,
    side TEXT NOT NULL,
    asset TEXT NOT NULL,
    amount TEXT NOT NULL,
    price TEXT NOT NULL,
    status TEXT NOT NULL,
    user_id TEXT NOT NULL,
    timestamp INTEGER NOT NULL,
    nonce TEXT NOT NULL
);
