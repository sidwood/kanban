CREATE TABLE voided_approvals (
    review_id INTEGER PRIMARY KEY,
    ticket_id INTEGER NOT NULL,
    tip TEXT NOT NULL
);
