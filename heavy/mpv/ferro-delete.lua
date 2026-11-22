-- ferro-delete: Delete in FerroOS's Video Player moves the video playing to
-- the Recycle Bin, after asking (one on a USB stick is written over and
-- deleted for good instead, never copied onto the computer). Only Y goes on; N, Esc, Enter and Delete
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

-- Another drive (a USB stick): never copied onto the computer to sit in
-- the Bin (a private video would then be on it too). It's written over and
-- deleted for good there instead, as the desktop and the file manager do.
local function wipe(path)
    local f = io.open(path, "r+b")
    if not f then return false end
    local size = f:seek("end") or 0
    f:seek("set", 0)
    local block = string.rep("\0", 1048576)
    local left = size
    while left > 0 do
        local n = math.min(left, #block)
        if not f:write(block:sub(1, n)) then f:close() return false end
        left = left - n
    end
    f:flush()
    f:close()
    return os.remove(path) ~= nil
end

-- On a USB stick or SD card (mounted where the desktop mounts them).
local function removable(path)
    return path:match("^/media/") ~= nil or path:match("^/run/media/") ~= nil
end

local function to_recycle_bin(path)
    local bin = (os.getenv("HOME") or "/tmp") .. "/.local/share/Trash"
    if not exists(bin) then return false, "there's no Recycle Bin folder" end
    local _, name = utils.split_path(path)
    local to = free_name(bin, name)
    if os.rename(path, to) then
        return true
    end
    if removable(path) and wipe(path) then
        return true, "gone"
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
    mp.osd_message(why == "gone" and "Deleted for good (it was on a removable drive)." or "Moved to the Recycle Bin.", 2)
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
    if removable(path) then
        mp.osd_message("'" .. name .. "' is on a removable drive, so it can't go to the Recycle Bin.\nDelete it for good? Press Y for Yes, or N to keep it.", 30)
    else
        mp.osd_message("Move '" .. name .. "' to the Recycle Bin?\nPress Y for Yes, or N to keep it.", 30)
    end
    for _, k in ipairs({ "y", "Y" }) do
        mp.add_forced_key_binding(k, "ferro-delete-" .. k, function() answer(true) end)
    end
    for _, k in ipairs({ "n", "N", "ESC", "ENTER", "DEL", "KP_ENTER" }) do
        mp.add_forced_key_binding(k, "ferro-delete-" .. k, function() answer(false) end)
    end
end

mp.add_key_binding(nil, "ask", ask)
