SELECT * FROM type_zoo;
SELECT id, c01 FROM wide_500k ORDER BY id;
SELECT * FROM wide_500k;
SELECT * FROM type_zoo WHERE id = 1;
SELECT nationkey, name, regionkey, comment FROM tpch.tiny.nation ORDER BY nationkey;
SELECT CAST(1234567890123456789012345678.1234567890 AS decimal(38,10)) AS high_precision, CAST(-0.0000000001 AS decimal(38,10)) AS tiny_negative, TIMESTAMP '2026-01-31 12:00:00.123456 +07:00' AS tz_aware, TIMESTAMP '2026-01-31 12:00:00.123456' AS tz_naive, DATE '2026-01-31' AS a_date, TIME '23:59:59.999999' AS a_time, INTERVAL '3' DAY + INTERVAL '4' HOUR + INTERVAL '5' MINUTE + INTERVAL '6' SECOND AS an_interval, CAST(NULL AS varchar) AS no_value, '' AS empty_text;
SELECT orderkey, totalprice, orderdate FROM tpch.tiny.orders ORDER BY orderkey;
SELECT * FROM tpch.tiny.orders;
SELECT * FROM tpch.tiny.nation;
