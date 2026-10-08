import 'dart:math';

// First.
int one(int x) {
  final y = x + 1;
  return y * 2;
}

/// Second.
@pragma('vm:prefer-inline')
int two(List<int> items) {
  var total = 0;
  for (final item in items) {
    total += item;
  }
  return total;
}
