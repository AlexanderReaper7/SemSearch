import os


# First.
def one(x):
    y = x + 1
    return y * 2


# Second.
@cache
def two(items):
    total = 0
    for item in items:
        total += item
    return total
