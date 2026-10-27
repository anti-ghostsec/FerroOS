-- ferro-delete: Delete in FerroOS's Video Player moves the video playing to
-- the Recycle Bin, after asking. Only Y goes on; N, Esc, Enter and Delete
-- again all keep it, so it can't go by a slip of the finger. The next video
-- in the folder plays in its place.
--
-- Plain Lua file operations only: inside ferro-jail nothing else may run.

local mp = require "mp"
local utils = require "mp.utils"

local asking = nil

local function exists(p)
    return utils.file_info(p) ~= nil
end

-- `name`, or `name (2)`, `name (3)`... whichever is free in `dir` (the way
-- the desktop and the file manager name things in the Recycle Bin).
local function free_name(dir, name)
    local p = dir .. "/" .. name
    if not exists(p) then return p end
    local stem, ext = name:match("^(.+)(%.[^.]+)$")
    if not stem then stem, ext = name, "" end
    local n = 2
    while true do
        p = string.format("%s/%s (%d)%s", dir, stem, n, ext)
        if not exists(p) then return p end
        n = n + 1
    end
end

-- Another drive (a USB stick): copied into the Bin, then removed.
local function copy_then_remove(from, to)
    local src = io.open(from, "rb")
    if not src then return false end
    local dst = io.open(to, "wb")
    if not dst then src:close() return false end
    while true do
        local block = src:read(1048576)
        if not block then break end
        if not dst:write(block) then
            src:close() dst:close() os.remove(to)
            return false
        end
    end
    src:close()
    if not dst:close() then os.remove(to) return false end
    return os.remove(from) ~= nil
end

local function to_recycle_bin(path)
    local bin = (os.getenv("HOME") or "/tmp") .. "/.local/share/Trash"
    if not exists(bin) then return false, "there's no Recycle Bin folder" end
    local _, name = utils.split_path(path)
    local to = free_name(bin, name)
    if os.rename(path, to) or copy_then_remove(path, to) then
        return true
    end
    return false, "it couldn't be moved (no permission, or a read-only drive)"
end

local function stop_asking()
    asking = nil
    for _, k in ipairs({ "y", "Y", "n", "N", "ESC", "ENTER", "DEL", "KP_ENTER" }) do
        mp.remove_key_binding("ferro-delete-" .. k)
    end
end

local function answer(yes)
    local path = asking
    stop_asking()
    if not yes or not path then
        mp.osd_message("Kept.", 1.5)
        return
    end
    local ok, why = to_recycle_bin(path)
    if not ok then
        mp.osd_message("Not deleted: " .. why .. ".", 4)
        return
    end
    mp.osd_message("Moved to the Recycle Bin.", 2)
    -- The next one plays; the last one closes the player.
    if mp.get_property_number("playlist-count", 1) > 1 then
        mp.commandv("playlist-remove", "current")
    else
        mp.command("quit")
    end
end

local function ask()
    local path = mp.get_property("path")
    if not path or path:find("://") then return end
    if not path:match("^/") then
        path = utils.join_path(mp.get_property("working-directory", ""), path)
    end
    asking = path
    local _, name = utils.split_path(path)
    mp.osd_message("Move '" .. name .. "' to the Recycle Bin?\nPress Y for Yes, or N to keep it.", 30)
    for _, k in ipairs({ "y", "Y" }) do
        mp.add_forced_key_binding(k, "ferro-delete-" .. k, function() answer(true) end)
    end
    for _, k in ipairs({ "n", "N", "ESC", "ENTER", "DEL", "KP_ENTER" }) do
        mp.add_forced_key_binding(k, "ferro-delete-" .. k, function() answer(false) end)
    end
end

mp.add_key_binding(nil, "ask", ask)
