-- Runs every case in cases.json through Neovim and writes what Neovim did to
-- expected.json: the reference omavim-vim is tested against.
--   nvim --headless --clean -l tools/fixtures/nvim.lua IN.json OUT.json
-- Each case: { name, text, cursor = {line, col} (0-based, col in chars), keys }
-- Keys are in Vim's notation ("d2w", "ciwX<Esc>", "<C-r>").

local input, output = arg[1], arg[2]
local cases = vim.json.decode(table.concat(vim.fn.readfile(input), "\n"))

local function char_col(line, byte_col)
  if byte_col >= #line then return vim.str_utfindex(line, "utf-32") end
  return vim.str_utfindex(line, "utf-32", byte_col)
end

local function byte_col(line, col)
  if col == 0 then return 0 end
  return vim.str_byteindex(line, "utf-32", math.min(col, vim.str_utfindex(line, "utf-32")))
end

local function pos(p) -- getpos() result -> {line, col} 0-based chars
  local line = vim.api.nvim_buf_get_lines(0, p[2] - 1, p[2], false)[1] or ""
  return { p[2] - 1, char_col(line, p[3] - 1) }
end

-- Registers recorded after each case (when not empty), besides "".
REGISTERS = "0123456789abcdefghijklmnopqrstuvwxyz-"

vim.o.report = 10000 -- no "3 fewer lines" messages
vim.o.shortmess = vim.o.shortmess .. "sIWF"
-- Without this, "-- INSERT --" after an error message waits a second for it
-- to be read.
vim.o.showmode = false
local results = {}
local function run(case)
  vim.cmd("enew!")
  vim.bo.bufhidden = "wipe"
  vim.api.nvim_buf_set_lines(0, 0, -1, false, vim.split(case.text, "\n", { plain = true }))
  vim.bo.modified = false
  for r in REGISTERS:gmatch(".") do vim.fn.setreg(r, {}) end
  vim.fn.setreg('"', "")
  -- No last search (setting "/ also forgets its offset), forwards.
  vim.fn.setreg("/", "")
  vim.v.searchforward = 1
  local first = vim.api.nvim_buf_get_lines(0, case.cursor[1], case.cursor[1] + 1, false)[1] or ""
  vim.api.nvim_win_set_cursor(0, { case.cursor[1] + 1, byte_col(first, case.cursor[2]) })
  -- A fresh undo history per case: the text set above isn't an undoable change.
  local undolevels = vim.bo.undolevels
  vim.bo.undolevels = -1
  vim.api.nvim_buf_set_lines(0, 0, 0, false, {})
  vim.bo.undolevels = undolevels

  -- The state is recorded by snap(), fed as the last key: <Cmd> runs in any
  -- mode without leaving it, so a case can end in insert or visual mode and be
  -- recorded as it is. (Waiting until the keys are done doesn't work headless:
  -- Neovim left in insert mode reads stdin, finds it closed, and quits.)
  local result
  _G.snap = function()
    local mode = vim.api.nvim_get_mode().mode
    local cur = vim.api.nvim_win_get_cursor(0)
    local line = vim.api.nvim_buf_get_lines(0, cur[1] - 1, cur[1], false)[1] or ""
    result = {
      name = case.name,
      text = table.concat(vim.api.nvim_buf_get_lines(0, 0, -1, false), "\n"),
      cursor = { cur[1] - 1, char_col(line, cur[2]) },
      mode = mode,
      register = vim.fn.getreg('"'),
      register_type = vim.fn.getregtype('"'),
      registers = vim.empty_dict(),
    }
    for r in (REGISTERS .. "/"):gmatch(".") do
      local text = vim.fn.getreg(r)
      if text ~= "" then result.registers[r] = { text, vim.fn.getregtype(r) } end
    end
    if mode == "v" or mode == "V" then
      result.visual_start = pos(vim.fn.getpos("v"))
    end
  end
  local keys = vim.api.nvim_replace_termcodes(case.keys .. "<Cmd>lua snap()<CR>", true, false, true)
  -- "t": as if typed, so Vim closes an undo step between commands, as it
  -- does for a person typing (without it, "dwdwu" undoes both deletes).
  local ok, err = pcall(vim.api.nvim_feedkeys, keys, "xt", false)
  if not result then
    -- A command failed, so Vim threw the rest of the keys away (snap with
    -- them). Record where that left things; the engine must stop there too.
    snap()
    result.aborted = true
    if not ok then result.error = tostring(err) end
  end
  table.insert(results, result)
  -- Back to normal mode for the next case.
  vim.api.nvim_feedkeys(vim.api.nvim_replace_termcodes("<Esc><Esc>", true, false, true), "x", false)
end

for i, case in ipairs(cases) do
  local ok, err = pcall(run, case)
  if not ok then
    io.stderr:write(("case %d (%s) failed: %s\n"):format(i, case.name, err))
    table.insert(results, { name = case.name, error = tostring(err) })
  end
end

local ok, json = pcall(vim.json.encode, results)
if not ok then
  io.stderr:write("could not encode the results: " .. json .. "\n")
  os.exit(1)
end
vim.fn.writefile({ json }, output)
io.stderr:write(("%d cases run\n"):format(#results))
