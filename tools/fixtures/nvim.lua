-- Runs every case in cases.json through Neovim and writes what Neovim did to
-- expected.json: the reference omavim-vim is tested against.
--   FIXTURE_IN=IN.json FIXTURE_OUT=OUT.json \
--     nvim --headless --clean -c "luafile tools/fixtures/nvim.lua" </dev/null
-- Each case: { name, text, cursor = {line, col} (0-based, col in chars), keys,
-- and optionally top = {line, rows} to start the view at }. Keys are in Vim's
-- notation ("d2w", "ciwX<Esc>", "<C-r>").
--
-- The keys are typed one at a time into Neovim's main loop, as a person types
-- them: Neovim redraws while it waits for each, and some of its state (the
-- view, above all) is only brought up to date then. Feeding them all at once
-- (nvim_feedkeys with "x") skips that and gives answers no one typing sees.

local input, output = os.getenv("FIXTURE_IN"), os.getenv("FIXTURE_OUT")
local cases = vim.json.decode(table.concat(vim.fn.readfile(input), "\n"))

local function char_col(line, byte_col)
  if byte_col >= #line then return vim.str_utfindex(line, "utf-32") end
  return vim.str_utfindex(line, "utf-32", byte_col)
end

local function byte_col(line, col)
  if col == 0 then return 0 end
  return vim.str_byteindex(line, "utf-32", math.min(col, vim.str_utfindex(line, "utf-32")))
end

-- A mark's byte column as chars, not moved back onto the line (a mark can
-- be past its line's end, or on a line that's gone).
local function mark_col(lnum, byte)
  local line = vim.api.nvim_buf_get_lines(0, lnum - 1, lnum, false)[1] or ""
  if byte >= #line then return vim.str_utfindex(line, "utf-32") + (byte - #line) end
  -- Part way into a char: that char (where Vim puts the cursor for it).
  byte = byte + vim.str_utf_start(line, byte + 1)
  return vim.str_utfindex(line, "utf-32", byte)
end

local function pos(p) -- getpos() result -> {line, col} 0-based chars
  local line = vim.api.nvim_buf_get_lines(0, p[2] - 1, p[2], false)[1] or ""
  return { p[2] - 1, char_col(line, p[3] - 1) }
end

-- A register's text as UTF-8: a recorded special key is Vim's K_SPECIAL
-- byte (0x80) and two more, and that byte alone isn't UTF-8. It's written
-- as U+0080, as the engine keeps it. (And <Ignore>, which Neovim records
-- when the harness's timer runs in the middle of a command, goes.)
local function utf8(s)
  s = s:gsub("\128\253\53", "")
  local out, i = {}, 1
  while i <= #s do
    if s:byte(i) == 0x80 and vim.str_utf_start(s, i) == 0 then
      -- (With its two bytes after it, each as the char of that number.)
      for j = i, math.min(i + 2, #s) do
        table.insert(out, vim.fn.nr2char(s:byte(j)))
      end
      i = i + 2
    else
      table.insert(out, s:sub(i, i))
    end
    i = i + 1
  end
  return table.concat(out)
end

-- Registers recorded after each case (when not empty), besides "".
local REGISTERS = "0123456789abcdefghijklmnopqrstuvwxyz-"

-- The window every case runs in: small, so lines wrap and the view scrolls.
-- tests/fixtures.rs gives the engine the same size.
local WIDTH, HEIGHT = 30, 8
vim.cmd("vsplit | split")
vim.api.nvim_win_set_width(0, WIDTH)
vim.api.nvim_win_set_height(0, HEIGHT)
vim.wo.linebreak = true
vim.wo.smoothscroll = true
vim.wo.number = false
vim.wo.signcolumn = "no"
vim.wo.foldcolumn = "0"

-- The view: the first line shown and how many of its rows are scrolled off.
local function top()
  local v = vim.fn.winsaveview()
  return { v.topline - 1, math.floor(v.skipcol / WIDTH) }
end

vim.o.report = 10000 -- no "3 fewer lines" messages
vim.o.shortmess = vim.o.shortmess .. "sIWF"
-- Without this, "-- INSERT --" after an error message waits a second for it
-- to be read.
vim.o.showmode = false
-- A key that starts one of Neovim's default mappings (visual "in", "an")
-- waits 'timeoutlen' for the rest before it's taken: don't wait long.
vim.o.timeoutlen = 1
-- The live preview of :s runs it on every key typed, making jumps no one
-- asked for.
vim.o.inccommand = ""

-- A case's keys, one by one, in the notation nvim_input takes: "<CR>" is one
-- key, and a "<" that doesn't start a key name is "<lt>".
local function split_keys(s)
  local keys = {}
  local i = 1
  while i <= #s do
    local c = s:sub(i, i)
    local chunk
    if c == "<" then
      local j = s:find(">", i, true)
      if j and not s:sub(i + 1, j - 1):find("<", 1, true) then
        local name = s:sub(i, j)
        if vim.api.nvim_replace_termcodes(name, true, false, true) ~= name then
          chunk = name
        end
      end
      if chunk then
        table.insert(keys, chunk)
        i = i + #chunk
      else
        table.insert(keys, "<lt>")
        i = i + 1
      end
    else
      local len = vim.str_utf_end(s, i) + 1
      table.insert(keys, s:sub(i, i + len - 1))
      i = i + len
    end
  end
  return keys
end

-- Keys Neovim has taken from its input, when it took the last, and how
-- many it had taken when it was last idle (SafeState: nothing pending, in
-- normal, insert, visual or command-line mode).
local taken, taken_at, idle = 0, 0, -1
vim.on_key(function()
  taken = taken + 1
  taken_at = vim.uv.now()
end, vim.api.nvim_create_namespace("fixtures"))
vim.api.nvim_create_autocmd("SafeState", { callback = function() idle = taken end })

local results = {}

local function start(case, done)
  -- (A case that ended while recording a macro doesn't record the next.)
  if vim.fn.reg_recording() ~= "" then vim.cmd("normal! q") end
  vim.cmd("enew!")
  vim.bo.bufhidden = "wipe"
  vim.api.nvim_buf_set_lines(0, 0, -1, false, vim.split(case.text, "\n", { plain = true }))
  vim.bo.modified = false
  for r in REGISTERS:gmatch(".") do vim.fn.setreg(r, {}) end
  vim.fn.setreg('"', "")
  -- No last search (setting "/ also forgets its offset), forwards.
  vim.fn.setreg("/", "")
  vim.v.searchforward = 1
  -- CTRL-D and CTRL-U with a count set 'scroll': back to half the window.
  vim.wo.scroll = 0
  -- Options :set can change, command and search history, and the last
  -- substitute (set to one that never matches: Neovim can't forget it).
  vim.o.ignorecase, vim.o.smartcase, vim.o.wrapscan, vim.o.hlsearch = false, false, true, true
  vim.o.tabstop, vim.o.shiftwidth = 8, 8
  vim.cmd([[silent! s/\%^\%$//e]])
  vim.fn.setreg("/", "")
  vim.fn.histdel(":")
  vim.fn.histdel("/")
  -- No marks or jumps from before; '' is where the case starts.
  vim.cmd("delmarks A-Z")
  local first = vim.api.nvim_buf_get_lines(0, case.cursor[1], case.cursor[1] + 1, false)[1] or ""
  vim.api.nvim_win_set_cursor(0, { case.cursor[1] + 1, byte_col(first, case.cursor[2]) })
  if case.top then
    -- (With the cursor's own column to aim for: without it, winrestview
    -- leaves the one from before.)
    vim.fn.winrestview({
      topline = case.top[1] + 1,
      skipcol = case.top[2] * WIDTH,
      curswant = vim.fn.winsaveview().curswant,
    })
  end
  -- The view as the keys find it: recorded by a first <Cmd>, once Neovim has
  -- redrawn (which can change what winrestview set).
  local start_top
  _G.start = function()
    start_top = top()
    vim.cmd("normal! m'")
    vim.cmd("clearjumps")
    -- (Setting up the undo history above counts as a change.)
    vim.cmd("delmarks .")
  end
  -- A fresh undo history per case: the text set above isn't an undoable change.
  local undolevels = vim.bo.undolevels
  vim.bo.undolevels = -1
  vim.api.nvim_buf_set_lines(0, 0, 0, false, {})
  vim.bo.undolevels = undolevels

  local result
  -- Records the state: fed as a last key, as <Cmd> runs in any mode without
  -- leaving it, so a case can end in insert or visual mode.
  _G.snap = function()
    local mode = vim.api.nvim_get_mode().mode
    local cur = vim.api.nvim_win_get_cursor(0)
    local line = vim.api.nvim_buf_get_lines(0, cur[1] - 1, cur[1], false)[1] or ""
    result = {
      name = case.name,
      text = utf8(table.concat(vim.api.nvim_buf_get_lines(0, 0, -1, false), "\n")),
      cursor = { cur[1] - 1, char_col(line, cur[2]) },
      mode = mode,
      register = utf8(vim.fn.getreg('"')),
      register_type = vim.fn.getregtype('"'),
      start_top = start_top,
      top = top(),
      registers = vim.empty_dict(),
    }
    for r in (REGISTERS .. "/"):gmatch(".") do
      local text = vim.fn.getreg(r)
      if text ~= "" then result.registers[r] = { utf8(text), vim.fn.getregtype(r) } end
    end
    if mode == "v" or mode == "V" then
      result.visual_start = pos(vim.fn.getpos("v"))
    end
    -- Marks that are set, and the jump list.
    result.marks = vim.empty_dict()
    for m in ("abAB'[].^<>"):gmatch(".") do
      local p = vim.fn.getpos("'" .. m)
      if p[2] > 0 then
        local col = p[3] >= 2147483647 and 2147483647 or mark_col(p[2], p[3] - 1)
        result.marks[m] = { p[2] - 1, col }
      end
    end
    local jl = vim.fn.getjumplist()
    local jumps = {}
    for _, j in ipairs(jl[1]) do
      table.insert(jumps, { j.lnum - 1, mark_col(j.lnum, j.col) })
    end
    result.jumps = { jumps, jl[2] }
  end

  local keys = split_keys(case.keys)
  table.insert(keys, 1, "<Cmd>lua start()<CR>")
  table.insert(keys, "<Cmd>lua snap()<CR>")
  -- Then back to normal mode for the next case, typed too (from a callback
  -- Esc would run inside insert mode, and not leave it).
  table.insert(keys, "<Esc>")
  table.insert(keys, "<Esc>")
  local fed = 0
  local base = taken
  local began = vim.uv.now()
  local timer = vim.uv.new_timer()
  local finished = false
  local function finish(r)
    if finished then return end
    finished = true
    timer:stop()
    timer:close()
    table.insert(results, r)
    vim.schedule(done)
  end
  -- Type the next key once Neovim has taken the last and waits for more.
  -- Having taken it isn't enough: a key typed while the last is still being
  -- handled is typeahead, and insert mode takes typeahead chars together.
  -- Neovim is waiting when it's idle, or when it's "blocking" for the rest
  -- of a command (after d, f, ", q); failing both for 50ms, type anyway.
  -- (Not "blocking" in insert mode: it's that too while it looks for more
  -- typed chars to insert with the last.)
  timer:start(0, 1, function()
    local m = vim.api.nvim_get_mode()
    local waiting = m.blocking and m.mode:sub(1, 1) ~= "i" and m.mode:sub(1, 1) ~= "R"
    local ready = taken - base >= fed
      and (idle == taken or waiting or vim.uv.now() - taken_at >= 50)
    if result and fed == #keys and taken - base >= fed then
      finish(result)
    elseif vim.uv.now() - began > 10000 then
      finish({ name = case.name, error = "timed out" })
    elseif fed < #keys and ready then
      fed = fed + 1
      vim.api.nvim_input(keys[fed])
    end
  end)
end

local i = 0
local function next_case()
  i = i + 1
  local case = cases[i]
  if not case then
    local ok, json = pcall(vim.json.encode, results)
    if not ok then
      io.stderr:write("could not encode the results: " .. json .. "\n")
      vim.cmd("cquit")
    end
    vim.fn.writefile({ json }, output)
    io.stderr:write(("%d cases run\n"):format(#results))
    vim.cmd("qa!")
    return
  end
  local ok, err = pcall(start, case, next_case)
  if not ok then
    io.stderr:write(("case %d (%s) failed: %s\n"):format(i, case.name, err))
    table.insert(results, { name = case.name, error = tostring(err) })
    vim.schedule(next_case)
  end
end
vim.schedule(next_case)
