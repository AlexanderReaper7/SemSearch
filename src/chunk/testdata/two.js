const fs = require("node:fs");

// First.
function one(x) {
  const y = x + 1;
  return y * 2;
}

/** Second.
 * Counts characters in a file.
 */
function two(path) {
  const text = fs.readFileSync(path, "utf8");
  let total = 0;
  for (const line of text.split("\n")) {
    total += line.length;
  }
  return total;
}

module.exports = { one, two };
