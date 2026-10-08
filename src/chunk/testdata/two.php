<?php

namespace Demo;

// First.
function one(int $x): int
{
    $y = $x + 1;
    return $y * 2;
}

/** Second. */
#[Pure]
function two(array $items): int
{
    $total = 0;
    foreach ($items as $item) {
        $total += $item;
    }
    return $total;
}
