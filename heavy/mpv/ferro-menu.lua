-- ferro-menu: the right-click menu of FerroOS's Video Player, a Windows 95
-- popup menu: grey and raised, at the pointer, the item under it in navy,
-- what can't be done now greyed out. Click an item, or use Up, Down and
-- Enter; Escape, a right-click or a click beside it closes it. Subtitles,
-- audio and video tracks, the other videos in the folder, chapters and audio
-- devices open as lists in its place. Stats are mpv's own (stats.lua).
-- Drawn only when it opens or the item under the pointer changes.

local mp = require "mp"

local overlay = mp.create_osd_overlay("ass-events")
local FONT = "DejaVu Sans"   -- the TrueType font FerroOS uses
-- Windows 95's colours, as ASS writes them (blue, green, red).
local FACE, LIGHT, WHITE = "C0C0C0", "DFDFDF", "FFFFFF"
local SHADOW, BLACK, NAVY = "808080", "000000", "800000"

-- What's open: { items = { { label, run, checked, disabled } }, hover,
-- scroll, x, y } ({ "-" } is a dividing line); and where it was drawn.
local open = nil
local geo = nil

local function scale()
    local hi = mp.get_property_number("display-hidpi-scale", 1) or 1
    return math.max(1, math.floor(hi + 0.5))
end

local function ab_label()
    local a, b = mp.get_property("ab-loop-a"), mp.get_property("ab-loop-b")
    if a == "no" then return "A-B loop: set A here" end
    if b == "no" then return "A-B loop: set B here" end
    return "A-B loop: off"
end

-- Info: what the video carries, to see, change or strip (ferro-info). The
-- player may start nothing else (ferro-jail), so the desktop opens it.
local function info()
    local path = mp.get_property("path")
    if not path or path:find("://") then return end
    mp.command_native_async({ name = "subprocess", playback_only = false, args = { "ferro-info", "--via-desktop", path } }, function() end)
end
mp.add_key_binding(nil, "info", info)

local function ass_escape(s)
    return (s:gsub("\\", "\\\\"):gsub("{", "\\{"):gsub("}", "\\}"))
end

local function rect(out, color, x, y, w, h)
    if w <= 0 or h <= 0 then return end
    out[#out + 1] = string.format("{\\an7\\pos(0,0)\\bord0\\shad0\\1c&H%s&\\p1}m %d %d l %d %d %d %d %d %d{\\p0}",
        color, x, y, x + w, y, x + w, y + h, x, y + h)
end

local function text(out, x, y, s, size, color, an)
    out[#out + 1] = string.format("{\\an%d\\pos(%d,%d)\\bord0\\shad0\\fn%s\\fs%d\\1c&H%s&}%s",
        an or 4, x, y, FONT, size, color, ass_escape(s))
end

-- The menu's size and place: by the pointer, kept inside the window.
local function layout()
    local w, h = mp.get_osd_size()
    if not w or w == 0 then return nil end
    local s = scale()
    local g = { s = s, row = 20 * s, sep = 8 * s, fs = 12 * s, ww = w, wh = h }
    local longest = 0
    for _, item in ipairs(open.items) do longest = math.max(longest, #item[1]) end
    g.w = math.max(150 * s, (longest * 7 + 48) * s)
    -- As many rows as fit; a long list (the videos in a folder) scrolls.
    local room = h - 8 * s
    local shown, used = 0, 6 * s
    for i = open.scroll + 1, #open.items do
        local add = open.items[i][1] == "-" and g.sep or g.row
        if used + add > room then break end
        used, shown = used + add, shown + 1
    end
    g.shown, g.h = shown, used
    g.x = math.max(0, math.min(open.x, w - g.w))
    g.y = math.max(0, math.min(open.y, h - g.h))
    return g
end

local function render()
    if not open then
        overlay:remove()
        geo = nil
        return
    end
    local g = layout()
    if not g then return end
    geo = g
    overlay.res_x, overlay.res_y = g.ww, g.wh
    local s, out = g.s, {}
    -- A raised panel, the Windows 95 way.
    -- (Its edges are thin strips, not stacked panels: without a graphics
    -- card each shape is blended into every frame, so less area is cheaper.)
    local x, y, w, h = g.x, g.y, g.w, g.h
    rect(out, FACE, x + 2 * s, y + 2 * s, w - 4 * s, h - 4 * s)
    rect(out, LIGHT, x, y, w - s, s)
    rect(out, LIGHT, x, y + s, s, h - 2 * s)
    rect(out, BLACK, x, y + h - s, w, s)
    rect(out, BLACK, x + w - s, y, s, h - s)
    rect(out, WHITE, x + s, y + s, w - 3 * s, s)
    rect(out, WHITE, x + s, y + 2 * s, s, h - 4 * s)
    rect(out, SHADOW, x + s, y + h - 2 * s, w - 2 * s, s)
    rect(out, SHADOW, x + w - 2 * s, y + s, s, h - 3 * s)
    g.rows = {}
    local y = g.y + 3 * s
    for k = 1, g.shown do
        local i = open.scroll + k
        local item = open.items[i]
        if item[1] == "-" then
            rect(out, SHADOW, g.x + 3 * s, y + 3 * s, g.w - 6 * s, s)
            rect(out, WHITE, g.x + 3 * s, y + 4 * s, g.w - 6 * s, s)
            y = y + g.sep
        else
            g.rows[#g.rows + 1] = { i = i, y = y }
            local cy = y + math.floor(g.row / 2)
            local colour = BLACK
            if open.hover == i and not item[4] then
                rect(out, NAVY, g.x + 3 * s, y, g.w - 6 * s, g.row)
                colour = WHITE
            end
            local label = item[1]
            if item[4] then
                -- Greyed out: shadow grey with a white emboss.
                text(out, g.x + 21 * s, cy + s, label, g.fs, WHITE)
                colour = SHADOW
            end
            if item[3] then text(out, g.x + 8 * s, cy, "•", g.fs, colour) end
            text(out, g.x + 20 * s, cy, label, g.fs, colour)
            -- A list of its own opens from it: an arrow at the right.
            if type(item[2]) == "function" and not item[5] then
                local ax = g.x + g.w - 12 * s
                out[#out + 1] = string.format("{\\an7\\pos(0,0)\\bord0\\shad0\\1c&H%s&\\p1}m %d %d l %d %d %d %d{\\p0}",
                    colour, ax, cy - 4 * s, ax + 4 * s, cy, ax, cy + 4 * s)
            end
            y = y + g.row
        end
    end
    overlay.data = table.concat(out, "\n")
    overlay:update()
end

-- The item at the pointer, if it's on the list.
local function item_at()
    if not open or not geo or not geo.rows then return nil end
    local x, y = mp.get_mouse_pos()
    if x < geo.x or x >= geo.x + geo.w then return nil end
    for _, r in ipairs(geo.rows) do
        if y >= r.y and y < r.y + geo.row then return r.i end
    end
    return nil
end

local function on_menu()
    if not open or not geo then return false end
    local x, y = mp.get_mouse_pos()
    return x >= geo.x and x < geo.x + geo.w and y >= geo.y and y < geo.y + geo.h
end

local function rows_shown() return geo and geo.shown or 10 end

local close

local function keep_in_sight()
    local shown = rows_shown()
    if open.hover then
        if open.hover <= open.scroll then open.scroll = open.hover - 1 end
        if open.hover > open.scroll + shown then open.scroll = open.hover - shown end
    end
    open.scroll = math.max(0, math.min(open.scroll, #open.items - shown))
end

local function pick(i)
    local item = open and i and open.items[i]
    if not item or item[1] == "-" or item[4] then return end
    local run = item[2]
    -- A list of its own (tracks, chapters...) replaces this one; anything
    -- else is done and the menu goes.
    if type(run) == "function" then
        if not run() then close() end
    else
        close()
        mp.command(run)
    end
end

local function bind(key, fn, repeatable)
    mp.add_forced_key_binding(key, "ferro_menu_" .. key, fn, { repeatable = repeatable })
end

local KEYS = { "MBTN_LEFT", "MBTN_LEFT_DBL", "MBTN_RIGHT", "ESC", "UP", "DOWN", "ENTER", "KP_ENTER", "WHEEL_UP", "WHEEL_DOWN" }

local function on_mouse()
    if not open then return end
    local i = item_at()
    if i ~= open.hover then
        open.hover = i
        render()
    end
end

close = function()
    open = nil
    for _, key in ipairs(KEYS) do mp.remove_key_binding("ferro_menu_" .. key) end
    mp.unobserve_property(on_mouse)
    mp.unobserve_property(render)
    render()
end

-- Up and Down go from item to item, past dividing lines and greyed ones.
local function step(dir)
    local i = open.hover or (dir > 0 and 0 or #open.items + 1)
    repeat
        i = i + dir
        local item = open.items[i]
        if item and item[1] ~= "-" and not item[4] then
            open.hover = i
            break
        end
    until not item
    keep_in_sight()
    render()
end

-- Opens a menu at the pointer; a list opened from it (tracks, chapters...)
-- takes its place there.
local function show(title, items)
    local first = open == nil
    local x, y = mp.get_mouse_pos()
    if not first then x, y = open.x, open.y end
    open = { title = title, items = items, hover = nil, scroll = 0, x = x, y = y }
    for i, item in ipairs(items) do
        if item[3] then
            open.hover = i
            break
        end
    end
    if first then
        -- The control bar (and its picture over the timeline) makes way.
        mp.commandv("script-message-to", "osc", "osc-hide")
        bind("MBTN_LEFT", function()
            local i = item_at()
            if i then pick(i) elseif not on_menu() then close() end
        end)
        bind("MBTN_LEFT_DBL", function() end)
        bind("MBTN_RIGHT", close)
        bind("ESC", close)
        bind("UP", function() step(-1) end, true)
        bind("DOWN", function() step(1) end, true)
        bind("ENTER", function() pick(open.hover) end)
        bind("KP_ENTER", function() pick(open.hover) end)
        bind("WHEEL_UP", function()
            open.scroll = math.max(0, open.scroll - 1)
            render()
        end, true)
        bind("WHEEL_DOWN", function()
            open.scroll = math.max(0, math.min(open.scroll + 1, #open.items - rows_shown()))
            render()
        end, true)
        mp.observe_property("mouse-pos", "native", on_mouse)
        mp.observe_property("osd-dimensions", "native", render)
    end
    keep_in_sight()
    render()
end

-- A list of the tracks of one kind (sub, audio, video), the one in use marked.
local function tracks(kind, title, prop)
    local items = {}
    if kind == "sub" then
        items[#items + 1] = { "Off", "set sid no", mp.get_property("sid") == "no" }
    end
    for _, t in ipairs(mp.get_property_native("track-list", {})) do
        if t.type == kind then
            local name = t.title or t.lang or ("Track " .. t.id)
            if t.lang and t.title then name = t.title .. " (" .. t.lang .. ")" end
            items[#items + 1] = { name, "set " .. prop .. " " .. t.id, t.selected }
        end
    end
    show(title, items)
    return true
end

local function playlist()
    local items = {}
    for i, entry in ipairs(mp.get_property_native("playlist", {})) do
        local name = entry.title or (entry.filename or ""):match("[^/]+$") or entry.filename
        items[#items + 1] = { name, "set playlist-pos " .. (i - 1), entry.current }
    end
    show("Videos in this folder", items)
    return true
end

local function chapters()
    local items = {}
    local now = mp.get_property_number("chapter", -1)
    for i, c in ipairs(mp.get_property_native("chapter-list", {})) do
        local t = math.floor(c.time or 0)
        local name = string.format("%d:%02d  %s", math.floor(t / 60), t % 60, c.title or ("Chapter " .. i))
        items[#items + 1] = { name, "set chapter " .. (i - 1), i - 1 == now }
    end
    show("Chapters", items)
    return true
end

local function audio_devices()
    local items = {}
    local now = mp.get_property("audio-device")
    for _, d in ipairs(mp.get_property_native("audio-device-list", {})) do
        items[#items + 1] = { d.description or d.name, "set audio-device " .. string.format("%q", d.name), d.name == now }
    end
    show("Audio device", items)
    return true
end

mp.add_key_binding(nil, "menu", function()
    if open then return close() end
    local subs, audios, videos = 0, 0, 0
    for _, track in pairs(mp.get_property_native("track-list", {})) do
        if track.type == "sub" then subs = subs + 1
        elseif track.type == "audio" then audios = audios + 1
        elseif track.type == "video" then videos = videos + 1 end
    end
    local has_path = mp.get_property("path") ~= nil
    local paused = mp.get_property_native("pause")
    -- { label, what it does, available?, no arrow? }; what isn't available
    -- now is greyed out, the Windows 95 way, so the menu keeps its shape.
    local menu = {
        { paused and "Play" or "Pause", "cycle pause", true },
        { "Full screen", "cycle fullscreen", true },
        { ab_label(), "ab-loop", has_path },
        { "-" },
        { "Subtitles", function() return tracks("sub", "Subtitles", "sid") end, subs > 0 },
        { "Audio track", function() return tracks("audio", "Audio track", "aid") end, audios > 1 },
        { "Video track", function() return tracks("video", "Video track", "vid") end, videos > 1 },
        { "Videos in this folder", playlist, mp.get_property_native("playlist-count", 0) > 1 },
        { "Chapters", chapters, mp.get_property("chapter") ~= nil },
        { "Audio device", audio_devices, audios > 0 },
        { "-" },
        { "Info", function() info() end, has_path, true },
        { "Stats", "script-binding stats/display-page-1-toggle", true },
        { "Keys", "script-binding stats/display-page-4-toggle", true },
        { "-" },
        { "Delete", "script-binding ferro_delete/ask", has_path },
    }
    local items = {}
    for _, entry in ipairs(menu) do
        items[#items + 1] = { entry[1], entry[2], false, entry[1] ~= "-" and not entry[3], entry[4] }
    end
    show("Menu", items)
end)
