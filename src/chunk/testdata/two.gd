extends Node

# First.
func one(x: int) -> int:
	var y = x + 1
	return y * 2

## Second.
@warning_ignore("unused_parameter")
func two(items: Array) -> int:
	var total = 0
	for item in items:
		total += item
	return total
