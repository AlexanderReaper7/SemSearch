package demo

import "sort"

// First.
func one(x int) int {
	y := x + 1
	return y * 2
}

// Second.
func two(items []int) int {
	sort.Slice(items, func(i, j int) bool {
		return items[i] < items[j]
	})
	total := 0
	for _, item := range items {
		total += item
	}
	return total
}
