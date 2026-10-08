local M = {}

-- First.
local function one(x)
  local y = x + 1
  return y * 2
end

--- Second.
function M.two(items)
  local total = 0
  for _, item in ipairs(items) do
    total = total + item
  end
  return total
end

return M
