-- Reconstructed from the catalog by QueryHive. Storage options, tablespace, ownership, grants, comments, triggers and rules are not included.
CREATE TABLE w11probe.child (
    id bigint GENERATED ALWAYS AS IDENTITY NOT NULL,
    parent_id bigint,
    doubled bigint GENERATED ALWAYS AS (id * 2) STORED,
    label text DEFAULT 'x'::text NOT NULL,
    CONSTRAINT child_pkey PRIMARY KEY (id),
    CONSTRAINT child_parent_id_fkey FOREIGN KEY (parent_id) REFERENCES w11probe.parent(id) ON DELETE CASCADE
);
CREATE INDEX child_parent_ix ON w11probe.child USING btree (parent_id) WHERE (id > 5);
