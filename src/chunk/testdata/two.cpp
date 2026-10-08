#include <vector>

namespace demo {

// First.
int one(int x) {
    int y = x + 1;
    return y * 2;
}

/// Second.
template <typename T>
T two(const std::vector<T>& items) {
    T total{};
    for (const auto& item : items) {
        total += item;
    }
    return total;
}

}  // namespace demo
