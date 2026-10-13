local M = {}

function M.make(who, loud)
  local s = "Hello, " .. who .. "."
  if loud then
    s = string.upper(s) .. "!"
  end
  return s
end

return M
