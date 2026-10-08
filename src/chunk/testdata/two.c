#include <stddef.h>

// First.
static int one(int x)
{
    int y = x + 1;
    return y * 2;
}

/* Second. */
int two(const int *items, size_t n)
{
    int total = 0;
    for (size_t i = 0; i < n; i++) {
        total += items[i];
    }
    return total;
}
