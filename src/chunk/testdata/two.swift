import Foundation

// First.
func one(_ x: Int) -> Int {
    let y = x + 1
    return y * 2
}

/// Second.
@discardableResult
func two(_ items: [Int]) -> Int {
    var total = 0
    for item in items {
        total += item
    }
    return total
}
