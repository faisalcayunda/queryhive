-- Reconstructed from the catalog by QueryHive. Storage options, tablespace, ownership, grants, comments, triggers and rules are not included.
CREATE VIEW w11probe."My View" AS
SELECT id,
    label
   FROM w11probe.child
  WHERE (id > 0);
