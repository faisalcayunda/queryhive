create function f(a int) returns int language plpgsql as $body$
begin
  -- a comment with a ; and a 'quote
  return a + 1;
end;
$body$;
select $$a;b$$, $x$ 'nested' $x$;
do $$ begin raise notice 'x'; end $$;
