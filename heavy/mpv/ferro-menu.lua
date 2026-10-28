-- ferro-menu: the menu of FerroOS's Video Player (right-click, or the ☰
-- button). A plain list down the left side of the picture, not over its
-- middle: click an item, or use Up, Down and Enter; Escape, a right-click
-- or a click beside it closes it. Subtitles, audio and video tracks, the
-- other videos in the folder, chapters and audio devices open as lists in
-- the same place. Stats are mpv's own (stats.lua).

local mp = require "mp"

local overlay = mp.create_osd_overlay("ass-events")
local WIDTH, ROW, FONT, TOP = 330, 34, 21, 24

-- What's open: { title, items = { { label, run, checked } }, hover, scroll }
local open = nil

local function ab_label()
    local a, b = mp.get_property("ab-loop-a"), mp.get_property("ab-loop-b")
    if a == "no" then return "A-B loop: set start" end
    if b == "no" then return "A-B loop: set end" end
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

local function rows_shown()
    local _, h = mp.get_osd_size()
    return math.max(3, math.floor(((h or 600) - TOP * 2) / ROW) - 1)
end

local function ass_escape(s)
    return (s:gsub("\\", "\\\\"):gsub("{", "\\{"):gsub("}", "\\}"))
end

local function rect(x, y, w, h, color, alpha)
    return string.format("{\\an7\\pos(%d,%d)\\bord0\\shad0\\1c&H%s&\\1a&H%s&\\p1}m 0 0 l %d 0 %d %d 0 %d{\\p0}", x, y, color, alpha, w, w, h, h)
end

local function text(x, y, s, color)
    return string.format("{\\an4\\pos(%d,%d)\\bord0\\shad0\\fs%d\\1c&H%s&}%s", x, y, FONT, color, ass_escape(s))
end

local function render()
    if not open then
        overlay:remove()
        return
    end
    local w, h = mp.get_osd_size()
    if not w or w == 0 then return end
    overlay.res_x, overlay.res_y = w, h
    local shown = math.min(#open.items, rows_shown())
    local panel_h = (shown + 1) * ROW + 8
    local out = {}
    out[#out + 1] = rect(0, TOP, WIDTH, panel_h, "1A1A1A", "28")
    out[#out + 1] = text(16, TOP + ROW / 2 + 2, open.title, "A0A0A0")
    for k = 1, shown do
        local i = open.scroll + k
        local item = open.items[i]
        if not item then break end
        local y = TOP + k * ROW + 4
        if open.hover == i then
            out[#out + 1] = rect(4, y, WIDTH - 8, ROW, "9A5C2E", "00")
        end
        local mark = item[3] and "•  " or "    "
        out[#out + 1] = text(16, y + ROW / 2, mark .. item[1], "FFFFFF")
    end
    overlay.data = table.concat(out, "\n")
    overlay:update()
end

-- The item at the pointer, if it's on the list.
local function item_at()
    if not open then return nil end
    local x, y = mp.get_mouse_pos()
    if x > WIDTH then return nil end
    local k = math.floor((y - TOP - 4) / ROW)
    local i = open.scroll + k
    if k >= 1 and k <= math.min(#open.items, rows_shown()) and open.items[i] then return i end
    return nil
end

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
    if not item then return end
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
    if i and i ~= open.hover then
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

local function show(title, items)
    local first = open == nil
    open = { title = title, items = items, hover = nil, scroll = 0 }
    for i, item in ipairs(items) do
        if item[3] then
            open.hover = i
            break
        end
    end
    if first then
        bind("MBTN_LEFT", function()
            local i = item_at()
            if i then pick(i) else close() end
        end)
        bind("MBTN_LEFT_DBL", function() end)
        bind("MBTN_RIGHT", close)
        bind("ESC", close)
        bind("UP", function()
            open.hover = math.max(1, (open.hover or 2) - 1)
            keep_in_sight()
            render()
        end, true)
        bind("DOWN", function()
            open.hover = math.min(#open.items, (open.hover or 0) + 1)
            keep_in_sight()
            render()
        end, true)
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
    local menu = {
        { ab_label(), "ab-loop", true },
        { "Subtitles", function() return tracks("sub", "Subtitles", "sid") end, subs > 0 },
        { "Audio track", function() return tracks("audio", "Audio track", "aid") end, audios > 1 },
        { "Video track", function() return tracks("video", "Video track", "vid") end, videos > 1 },
        { "Videos in this folder", playlist, mp.get_property_native("playlist-count", 0) > 1 },
        { "Chapters", chapters, mp.get_property("chapter") ~= nil },
        { "Audio device", audio_devices, audios > 0 },
        { "Info", function() info() end, has_path },
        { "Stats", "script-binding stats/display-page-1-toggle", true },
        { "Keys", "script-binding stats/display-page-4-toggle", true },
        { "Delete", "script-binding ferro_delete/ask", has_path },
    }
    local items = {}
    for _, entry in ipairs(menu) do
        if entry[3] then items[#items + 1] = { entry[1], entry[2] } end
    end
    show("Menu", items)
end)
