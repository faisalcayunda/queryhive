SELECT count(*) FROM payments WHERE region IN (:a, :b, :c); -- trailing comment 0

-- statement 1: recent sessions
SELECT region, note, status
  FROM sessions
 WHERE region > :min_id AND status = 'active'
 ORDER BY created_at DESC LIMIT 100;

INSERT INTO sessions (region, amount, status)
VALUES (450671, 'a;b', 0.0268),
       (41814, 'c -- d', 0.4973);

CREATE OR REPLACE FUNCTION f_3() RETURNS text AS $body$
BEGIN RETURN 'x' || '3'; -- not a real comment end; $$ inside is fine
END; $body$ LANGUAGE plpgsql;
SELECT $tag$orders 'quoted' "and" -- text$tag$;

WITH recent AS (
  SELECT note, sum(amount) AS total /* running total */
    FROM events WHERE created_at >= :since GROUP BY note
), ranked AS (
  SELECT *, rank() OVER (ORDER BY total DESC) AS rk FROM recent
)
SELECT * FROM ranked WHERE rk <= :top_n;

INSERT INTO orders (created_at, amount, status)
VALUES (834063, 'a;b', 0.5449),
       (212459, 'c -- d', 0.7440);

/* block comment 6
   spanning several lines, with 'quotes' and -- dashes inside */
UPDATE sessions SET note = 'it''s 6', status = "done" WHERE created_at = :id;

WITH recent AS (
  SELECT status, sum(amount) AS total /* running total */
    FROM sessions WHERE created_at >= :since GROUP BY status
), ranked AS (
  SELECT *, rank() OVER (ORDER BY total DESC) AS rk FROM recent
)
SELECT * FROM ranked WHERE rk <= :top_n;

SELECT count(*) FROM orders WHERE status IN (:a, :b, :c); -- trailing comment 8

INSERT INTO sessions (created_at, amount, region)
VALUES (343129, 'a;b', 0.8760),
       (200382, 'c -- d', 0.3430);

INSERT INTO events (amount, user_id, id)
VALUES (264671, 'a;b', 0.4357),
       (483564, 'c -- d', 0.8175);

INSERT INTO events (status, region, id)
VALUES (882216, 'a;b', 0.2972),
       (723286, 'c -- d', 0.7326);

CREATE OR REPLACE FUNCTION f_12() RETURNS text AS $body$
BEGIN RETURN 'x' || '12'; -- not a real comment end; $$ inside is fine
END; $body$ LANGUAGE plpgsql;
SELECT $tag$orders 'quoted' "and" -- text$tag$;

/* block comment 13
   spanning several lines, with 'quotes' and -- dashes inside */
UPDATE payments SET note = 'it''s 13', status = "done" WHERE region = :id;

SELECT id_0 AS c0, region_1 AS c1, user_id_2 AS c2, created_at_3 AS c3, id_4 AS c4, region_5 AS c5, id_6 AS c6, status_7 AS c7, status_8 AS c8, status_9 AS c9, created_at_10 AS c10, status_11 AS c11, user_id_12 AS c12, note_13 AS c13, amount_14 AS c14, amount_15 AS c15, region_16 AS c16, region_17 AS c17, region_18 AS c18, note_19 AS c19, created_at_20 AS c20, region_21 AS c21, id_22 AS c22, note_23 AS c23, created_at_24 AS c24, region_25 AS c25, amount_26 AS c26, region_27 AS c27, note_28 AS c28, amount_29 AS c29, amount_30 AS c30, note_31 AS c31, id_32 AS c32, region_33 AS c33, id_34 AS c34, amount_35 AS c35, id_36 AS c36, note_37 AS c37, status_38 AS c38, created_at_39 AS c39, created_at_40 AS c40, user_id_41 AS c41, amount_42 AS c42, created_at_43 AS c43, created_at_44 AS c44, created_at_45 AS c45, user_id_46 AS c46, status_47 AS c47, note_48 AS c48, note_49 AS c49, note_50 AS c50, note_51 AS c51, id_52 AS c52, id_53 AS c53, note_54 AS c54, id_55 AS c55, amount_56 AS c56, created_at_57 AS c57, note_58 AS c58, region_59 AS c59 FROM orders WHERE note LIKE '%xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx%';

WITH recent AS (
  SELECT note, sum(amount) AS total /* running total */
    FROM sessions WHERE created_at >= :since GROUP BY note
), ranked AS (
  SELECT *, rank() OVER (ORDER BY total DESC) AS rk FROM recent
)
SELECT * FROM ranked WHERE rk <= :top_n;

-- statement 16: recent events
SELECT created_at, id, status
  FROM events
 WHERE created_at > :min_id AND status = 'active'
 ORDER BY created_at DESC LIMIT 100;

