-- Reconstructed from the catalog by QueryHive. Storage options, tablespace, ownership, grants, comments, triggers and rules are not included.
CREATE TABLE w11probe.events (
    d date NOT NULL
) PARTITION BY RANGE (d);
