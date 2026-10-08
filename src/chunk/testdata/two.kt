package demo

// First.
fun one(x: Int): Int {
    val y = x + 1
    return y * 2
}

/** Second. */
@Deprecated("use sum")
fun two(items: List<Int>): Int {
    var total = 0
    for (item in items) {
        total += item
    }
    return total
}
