-module(numbers).
-export([one/1, two/1]).

% First.
one(X) ->
    Y = X + 1,
    Y * 2.

%% Second.
-spec two([integer()]) -> integer().
two(Items) ->
    lists:foldl(fun(Item, Total) ->
        Total + Item
    end, 0, Items).
