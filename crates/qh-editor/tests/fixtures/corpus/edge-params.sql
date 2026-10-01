select a::text, :b, :=, x:a, arr[lo:hi], arr[1:n], arr[:n], (:a), ':a', :a
-- :a
;
select $$ :a $$, :a
;
:a;
select 1 where x = :since and y in (:a, :b_2, :c3) and z = ?;
