require "set"

class Numbers
  # First.
  def one(x)
    y = x + 1
    y * 2
  end

  # Second.
  def two(items)
    total = 0
    items.each do |item|
      total += item
    end
    total
  end
end
