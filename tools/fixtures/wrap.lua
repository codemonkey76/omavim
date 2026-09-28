-- Records where Neovim puts each char of a line on screen, with 'linebreak'
-- and 'wrap' on, in a window of the case's width. For wrap_cases.json:
--   nvim --headless --clean -l tools/fixtures/wrap.lua IN.json OUT.json
local input, output = arg[1], arg[2]
local cases = vim.json.decode(table.concat(vim.fn.readfile(input), "\n"))
vim.cmd("vsplit | split")
vim.wo.linebreak = true
vim.wo.number = false
vim.wo.signcolumn = "no"
vim.wo.foldcolumn = "0"
local results = {}
for _, case in ipairs(cases) do
  vim.api.nvim_win_set_width(0, case.width)
  vim.api.nvim_buf_set_lines(0, 0, -1, false, { case.line })
  local cells = {}
  local chars = vim.fn.strchars(case.line)
  for i = 0, chars - 1 do
    local byte = vim.fn.byteidx(case.line, i) + 1
    -- The char's first screen column, counting 'linebreak' padding.
    local v = vim.fn.virtcol({ 1, byte }, 1)[1] - 1
    table.insert(cells, { math.floor(v / case.width), v % case.width })
  end
  table.insert(results, { width = vim.api.nvim_win_get_width(0), cells = cells })
end
vim.fn.writefile({ vim.json.encode(results) }, output)
io.stderr:write(("%d wrap cases run\n"):format(#results))
