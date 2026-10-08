defmodule Demo.Numbers do
  # First.
  def one(x) do
    y = x + 1
    y * 2
  end

  # Second.
  @doc """
  Sums the items.
  """
  @spec two([integer]) :: integer
  def two(items) do
    Enum.reduce(items, 0, fn item, total ->
      total + item
    end)
  end
end
