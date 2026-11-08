-- FerroOS Video Player's control bar, drawn like Windows 95: one grey row
-- along the bottom with bevelled buttons (Play/Pause, Stop, Previous, Next,
-- Loop, A-B), a classic trackbar for the timeline, the time in a sunken box,
-- a speaker button with a volume trackbar, and Full screen; tooltips in
-- pale yellow.
--
-- FerroOS's own, written to be light (it replaces mpv's osc.lua, which
-- ferro-video turns off with --osc=no):
--   * nothing runs while the bar is hidden: no timers, no redraws; moving
--     the mouse shows it, and it hides 1.5 seconds after the mouse stops
--     (not while the pointer is on the bar);
--   * while it shows, the time is looked at four times a second and the bar
--     is only drawn again when something on it changed;
--   * the whole bar is one overlay of plain rectangles and a few words,
--     kept low (32 pixels): without a graphics card mpv blends whatever it
--     covers into every frame.
-- Hovering the timeline shows that moment's time, and ferro-thumbs.lua
-- draws its picture above it. Mouse clicks on the bar work its buttons;
-- anywhere else they do what input.conf says (a click pauses).

local mp = require "mp"

local overlay = mp.create_osd_overlay("ass-events")

-- Windows 95's colours, as ASS writes them (blue, green, red).
local FACE, LIGHT, WHITE = "C0C0C0", "DFDFDF", "FFFFFF"
local SHADOW, BLACK, NAVY = "808080", "000000", "800000"
local TIP = "E1FFFF"         -- tooltip yellow (#FFFFE1)
local FONT = "DejaVu Sans"   -- the TrueType font FerroOS uses (notifications too)

local BAR_H = 32             -- the bar's height, in pixels
local HIDE_AFTER = 1.5       -- seconds without mouse movement
local TICK = 0.25            -- how often the time is looked at while shown

-- State.
local shown = false
local mouse = { x = -1, y = -1, inside = false }
local over_bar = false
local hide_timer = nil
local tick_timer = nil
local drag = nil             -- "seek" or "volume" while the button is held
local pressed = nil          -- the button held down
local tip_ready = false      -- a button's tooltip waits a moment, like Windows
local tip_timer = nil
local last_data = nil
local bindings_on = false
local saved_autohide = nil
local thumb_sent = nil
local hits = {}              -- what's where, from the last drawing
local prev_hits = {}         -- the one before (while drawing the next)

---------------------------------------------------------------- drawing

local out              -- the drawing being built: a list of ASS events
local S = 1            -- the scale it's drawn at

-- Each shape is an event of its own: libass makes a bitmap the size of each
-- event's outline, and without a graphics card mpv blends those into every
-- frame, so a few small shapes cost far less than one spread across the bar.
local function shape(colour, path)
    out[#out + 1] = "{\\an7\\pos(0,0)\\bord0\\shad0\\1c&H" .. colour .. "&\\p1}" .. path .. "{\\p0}"
end

-- A filled rectangle.
local function box(colour, x, y, w, h)
    if w <= 0 or h <= 0 then return end
    shape(colour, string.format("m %d %d l %d %d %d %d %d %d", x, y, x + w, y, x + w, y + h, x, y + h))
end

-- Any filled shape, from a list of points.
local function poly(colour, pts)
    local s = { string.format("m %d %d l", pts[1], pts[2]) }
    for i = 3, #pts, 2 do s[#s + 1] = string.format("%d %d", pts[i], pts[i + 1]) end
    shape(colour, table.concat(s, " "))
end

local function ass_escape(s)
    return (s:gsub("\\", "\\\\"):gsub("{", "\\{"):gsub("}", "\\}"))
end

-- an: ASS alignment (4 = left middle, 5 = centre, 6 = right middle).
local function text(x, y, s, size, colour, an)
    out[#out + 1] = string.format("{\\an%d\\pos(%d,%d)\\bord0\\shad0\\fn%s\\fs%d\\1c&H%s&}%s",
        an or 4, x, y, FONT, size, colour or BLACK, ass_escape(s))
end

-- Windows 95's edges: two lines of light and shadow on each side, raised
-- (a button) or sunken (a field, a button held down or switched on). The
-- inside is only filled when it isn't the panel's own grey (`fill`).
local function edge(x, y, w, h, tl_out, br_out, tl_in, br_in, fill)
    local s = S
    if fill then box(fill, x + 2 * s, y + 2 * s, w - 4 * s, h - 4 * s) end
    box(tl_out, x, y, w - s, s)
    box(tl_out, x, y + s, s, h - 2 * s)
    box(br_out, x, y + h - s, w, s)
    box(br_out, x + w - s, y, s, h - s)
    box(tl_in, x + s, y + s, w - 3 * s, s)
    box(tl_in, x + s, y + 2 * s, s, h - 4 * s)
    box(br_in, x + s, y + h - 2 * s, w - 2 * s, s)
    box(br_in, x + w - 2 * s, y + s, s, h - 3 * s)
end

local function raised(x, y, w, h, fill)
    edge(x, y, w, h, WHITE, BLACK, LIGHT, SHADOW, fill)
end

local function sunken(x, y, w, h, fill)
    edge(x, y, w, h, SHADOW, WHITE, BLACK, LIGHT, fill)
end

---------------------------------------------------------------- icons

-- Each draws centred on (cx, cy) in `c`, at scale `s`.
local icons = {}
function icons.play(cx, cy, c, s)
    poly(c, { cx - 3 * s, cy - 5 * s, cx + 3 * s, cy, cx - 3 * s, cy + 5 * s })
end
function icons.pause(cx, cy, c, s)
    box(c, cx - 4 * s, cy - 5 * s, 3 * s, 10 * s)
    box(c, cx + 1 * s, cy - 5 * s, 3 * s, 10 * s)
end
function icons.stop(cx, cy, c, s)
    box(c, cx - 4 * s, cy - 4 * s, 8 * s, 8 * s)
end
function icons.prev(cx, cy, c, s)
    box(c, cx - 5 * s, cy - 4 * s, 2 * s, 8 * s)
    poly(c, { cx + 3 * s, cy - 4 * s, cx + 3 * s, cy + 4 * s, cx - 3 * s, cy })
end
function icons.next(cx, cy, c, s)
    poly(c, { cx - 3 * s, cy - 4 * s, cx + 3 * s, cy, cx - 3 * s, cy + 4 * s })
    box(c, cx + 3 * s, cy - 4 * s, 2 * s, 8 * s)
end
function icons.speaker(cx, cy, c, s, muted)
    box(c, cx - 6 * s, cy - 2 * s, 3 * s, 5 * s)
    poly(c, { cx - 3 * s, cy - 2 * s, cx + 1 * s, cy - 6 * s, cx + 1 * s, cy + 7 * s, cx - 3 * s, cy + 3 * s })
    if muted then
        -- A small x beside it.
        poly(c, { cx + 3 * s, cy - 2 * s, cx + 4 * s, cy - 3 * s, cx + 8 * s, cy + 2 * s, cx + 7 * s, cy + 3 * s })
        poly(c, { cx + 3 * s, cy + 2 * s, cx + 7 * s, cy - 3 * s, cx + 8 * s, cy - 2 * s, cx + 4 * s, cy + 3 * s })
    else
        box(c, cx + 3 * s, cy - 1 * s, 1 * s, 3 * s)
        box(c, cx + 5 * s, cy - 3 * s, 1 * s, 7 * s)
    end
end
-- A little window: Full screen (and back).
function icons.window(cx, cy, c, s, small)
    local w, h = (small and 9 or 13) * s, (small and 7 or 10) * s
    local x, y = cx - math.floor(w / 2), cy - math.floor(h / 2)
    box(c, x, y, w, h)
    box(WHITE, x + s, y + 3 * s, w - 2 * s, h - 4 * s)
    if c == BLACK then box(NAVY, x + s, y + s, w - 2 * s, 2 * s) end
end

---------------------------------------------------------------- the bar

local function fmt_time(t)
    t = math.max(0, math.floor(t or 0))
    local h, m, sec = math.floor(t / 3600), math.floor(t / 60) % 60, t % 60
    if h > 0 then return string.format("%d:%02d:%02d", h, m, sec) end
    return string.format("%d:%02d", m, sec)
end

local function scale()
    local hi = mp.get_property_number("display-hidpi-scale", 1) or 1
    return math.max(1, math.floor(hi + 0.5))
end

local function inside(r, x, y)
    return r and x >= r.x and x < r.x + r.w and y >= r.y and y < r.y + r.h
end

-- A button: raised, or sunken when held down or switched on (Loop, A-B).
local function button(name, x, y, w, h, s, draw_icon, opts)
    opts = opts or {}
    local down = pressed == name and inside(prev_hits[name], mouse.x, mouse.y) or opts.on
    if down then
        sunken(x, y, w, h, opts.on and LIGHT or nil)
    else
        raised(x, y, w, h)
    end
    local cx, cy = x + math.floor(w / 2), y + math.floor(h / 2)
    if down then cx, cy = cx + 1, cy + 1 end
    if opts.disabled then
        -- Greyed out the Windows 95 way: shadow grey with a white emboss.
        draw_icon(cx + s, cy + s, WHITE)
        draw_icon(cx, cy, SHADOW)
    else
        draw_icon(cx, cy, BLACK)
    end
    hits[name] = { x = x, y = y, w = w, h = h, disabled = opts.disabled, tip = opts.tip }
end

local function render()
    if not shown then return end
    local w, h = mp.get_osd_size()
    if not w or w == 0 then return end
    local s = scale()
    S = s
    local fs = 12 * s
    out = {}
    prev_hits, hits = hits, {}

    local idle = mp.get_property_native("idle-active")
    local duration = mp.get_property_number("duration", 0) or 0
    local pos = mp.get_property_number("time-pos", 0) or 0
    local paused = mp.get_property_native("pause")
    local count = mp.get_property_number("playlist-count", 0) or 0
    local volume = mp.get_property_number("volume", 0) or 0
    local muted = mp.get_property_native("mute")
    local a, b = mp.get_property_number("ab-loop-a"), mp.get_property_number("ab-loop-b")
    local looping = mp.get_property("loop-file") ~= "no"
    local fullscreen = mp.get_property_native("fullscreen")

    -- The panel: one row, as low as the buttons allow (without a graphics
    -- card, every pixel it covers is blended into every frame).
    local bh = BAR_H * s
    local top = h - bh
    box(WHITE, 0, top, w, s)
    box(LIGHT, 0, top + s, w, s)
    box(FACE, 0, top + 2 * s, w, bh - 2 * s)

    -- At the left, the buttons.
    local row = top + 5 * s
    local bw, bhh, gap = 24 * s, 22 * s, 2 * s
    local x = 6 * s
    local no_file = idle or duration <= 0
    local where = count > 1 and string.format(" (%d of %d)", (mp.get_property_number("playlist-pos", 0) or 0) + 1, count) or ""
    button("play", x, row, bw, bhh, s, function(cx, cy, c)
        if paused or idle then icons.play(cx, cy, c, s) else icons.pause(cx, cy, c, s) end
    end, { tip = paused and "Play" or "Pause", disabled = idle })
    x = x + bw + gap
    button("stop", x, row, bw, bhh, s, function(cx, cy, c) icons.stop(cx, cy, c, s) end,
        { tip = "Stop", disabled = no_file })
    x = x + bw + 6 * s
    button("prev", x, row, bw, bhh, s, function(cx, cy, c) icons.prev(cx, cy, c, s) end,
        { tip = "Previous video" .. where, disabled = count < 2 })
    x = x + bw + gap
    button("next", x, row, bw, bhh, s, function(cx, cy, c) icons.next(cx, cy, c, s) end,
        { tip = "Next video" .. where, disabled = count < 2 })
    x = x + bw + 6 * s
    local lw = 38 * s
    button("loop", x, row, lw, bhh, s, function(cx, cy, c) text(cx, cy, "Loop", fs, c, 5) end,
        { on = looping, tip = looping and "Looping: click to play on to the next" or "Loop this video" })
    x = x + lw + gap
    button("ab", x, row, lw, bhh, s, function(cx, cy, c) text(cx, cy, "A-B", fs, c, 5) end,
        { on = a ~= nil, disabled = no_file,
          tip = (not a and "A-B loop: set A here") or (not b and "A-B loop: set B here (right-click: off)")
              or "A-B loop: off (right-click too)" })
    x = x + lw + 6 * s

    -- At the right: Full screen, the volume trackbar, the speaker, and the
    -- time in a sunken box.
    local rx = w - 6 * s - bw
    button("fullscreen", rx, row, bw, bhh, s, function(cx, cy, c) icons.window(cx, cy, c, s, fullscreen) end,
        { tip = fullscreen and "Leave full screen" or "Full screen", on = fullscreen })
    local tw = (duration >= 3600 and 120 or 90) * s
    local vw = 60 * s
    local vx = rx - 8 * s - vw
    -- The volume trackbar only when there's room for it and a timeline.
    if vx - 4 * s - bw - 6 * s - tw - 6 * s > x + 120 * s then
        local vy = row + 9 * s
        box(SHADOW, vx, vy, vw, s)
        box(WHITE, vx, vy + 3 * s, vw, s)
        box(BLACK, vx + s, vy + s, vw - 2 * s, s)
        box(LIGHT, vx + s, vy + 2 * s, vw - 2 * s, s)
        local v = math.min(100, math.max(0, volume))
        local vpx = vx + math.floor(v / 100 * (vw - 1))
        box(NAVY, vx + s, vy + s, vpx - vx - s, 2 * s)
        raised(vpx - 3 * s, row + 2 * s, 7 * s, 18 * s, FACE)
        hits.volume = { x = vx - 4 * s, y = row, w = vw + 8 * s, h = bhh, tip = string.format("Volume %d%%", volume) }
        rx = vx - 6 * s - bw
    else
        rx = rx - gap - bw
    end
    button("mute", rx, row, bw, bhh, s, function(cx, cy, c) icons.speaker(cx, cy, c, s, muted or volume == 0) end,
        { tip = muted and "Sound off: click for sound" or "Sound on: click to turn off", on = muted })
    local tx = rx - 6 * s - tw
    sunken(tx, row + s, tw, 20 * s, WHITE)
    text(tx + math.floor(tw / 2), row + 11 * s, fmt_time(pos) .. " / " .. fmt_time(duration), fs, BLACK, 5)

    -- In between, the timeline: a trackbar.
    local gx = x + 8 * s
    local gw = tx - 8 * s - gx
    local gy = row + 8 * s
    local track = { x = gx - 6 * s, y = row, w = gw + 12 * s, h = bhh }
    hits.seek = track
    if gw > 20 * s then
        -- The groove (sunken), white, the part played so far navy.
        box(SHADOW, gx, gy, gw, s)
        box(WHITE, gx, gy + 3 * s, gw, s)
        box(BLACK, gx + s, gy + s, gw - 2 * s, s)
        box(LIGHT, gx + s, gy + 2 * s, gw - 2 * s, s)
        local frac = duration > 0 and math.min(1, pos / duration) or 0
        local px = gx + math.floor(frac * gw)
        local inner = gx + s
        if a and b and duration > 0 then
            -- The A-B loop: the stretch between the two marks.
            local ax, bx = gx + math.floor(a / duration * gw), gx + math.floor(b / duration * gw)
            box(NAVY, math.min(ax, bx), gy + 2 * s, math.abs(bx - ax), s)
        else
            box(NAVY, inner, gy + s, px - inner, 2 * s)
        end
        -- Tick marks under the groove at chapters, and A and B.
        for _, c in ipairs(mp.get_property_native("chapter-list", {})) do
            if duration > 0 and c.time then
                box(BLACK, gx + math.floor(c.time / duration * gw), gy + 9 * s, s, 3 * s)
            end
        end
        for _, m in ipairs({ a, b }) do
            if m and duration > 0 then
                local mx = gx + math.floor(m / duration * gw)
                poly(NAVY, { mx, gy + 7 * s, mx + 4 * s, gy + 12 * s, mx - 4 * s, gy + 12 * s })
            end
        end
        -- The thumb: a raised slab across the groove.
        if duration > 0 and not idle then
            raised(px - 5 * s, row, 11 * s, bhh, FACE)
        end
    end

    -- The time at the pointer over the timeline, in a tooltip above it
    -- (ferro-thumbs draws the picture above that), or a button's name.
    local tip, tip_x = nil, mouse.x
    if (inside(track, mouse.x, mouse.y) or drag == "seek") and duration > 0 and gw > 20 * s then
        local t = math.max(0, math.min(1, (mouse.x - gx) / gw)) * duration
        tip = fmt_time(t)
        local ty = top - 22 * s
        if thumb_sent ~= tip then
            thumb_sent = tip
            mp.commandv("script-message-to", "ferro_thumbs", "thumb", string.format("%.2f", t),
                tostring(math.floor(mouse.x)), tostring(ty - 2 * s))
        end
    else
        if thumb_sent then
            thumb_sent = nil
            mp.commandv("script-message-to", "ferro_thumbs", "thumb-hide")
        end
        if tip_ready and not drag then
            for _, r in pairs(hits) do
                if r.tip and inside(r, mouse.x, mouse.y) then tip = r.tip end
            end
        end
    end
    if tip then
        local tw2 = (#tip * 7 + 10) * s
        local tx2 = math.max(2 * s, math.min(tip_x - math.floor(tw2 / 2), w - tw2 - 2 * s))
        local ty = top - 22 * s
        box(BLACK, tx2, ty, tw2, 18 * s)
        box(TIP, tx2 + s, ty + s, tw2 - 2 * s, 16 * s)
        text(tx2 + math.floor(tw2 / 2), ty + 9 * s, tip, fs, BLACK, 5)
    end

    local data = table.concat(out, "\n")
    if data ~= last_data or overlay.res_x ~= w or overlay.res_y ~= h then
        last_data = data
        overlay.res_x, overlay.res_y = w, h
        overlay.data = data
        overlay:update()
    end
end

---------------------------------------------------------------- clicks

local function seek_to(x, exact)
    local r = hits.seek
    local duration = mp.get_property_number("duration")
    if not r or not duration or duration <= 0 then return end
    local s = scale()
    local gx, gw = r.x + 6 * s, r.w - 12 * s
    local frac = math.max(0, math.min(1, (x - gx) / gw))
    -- Exact where it's clicked or let go; by keyframes while dragging, to keep up.
    mp.commandv("seek", tostring(frac * duration), exact and "absolute+exact" or "absolute+keyframes")
end

local function volume_to(x)
    local r = hits.volume
    if not r then return end
    local s = scale()
    local v = math.max(0, math.min(100, (x - r.x - 4 * s) / (r.w - 8 * s - 1) * 100))
    mp.set_property_number("volume", math.floor(v + 0.5))
    if v > 0 then mp.set_property_native("mute", false) end
end

local ACTIONS = {
    play = function() mp.command("cycle pause") end,
    -- Stop, the Windows 95 way: back to the start, paused.
    stop = function()
        mp.set_property_native("pause", true)
        mp.commandv("seek", "0", "absolute+exact")
    end,
    prev = function() mp.command("playlist-prev") end,
    next = function() mp.command("playlist-next") end,
    loop = function() mp.command("cycle-values loop-file inf no") end,
    ab = function() mp.command("ab-loop") end,
    mute = function() mp.command("cycle mute") end,
    fullscreen = function() mp.command("cycle fullscreen") end,
}

local function on_click(t)
    local x, y = mouse.x, mouse.y
    if t.event == "down" then
        if inside(hits.seek, x, y) then
            drag = "seek"
            seek_to(x, true)
        elseif inside(hits.volume, x, y) then
            drag = "volume"
            volume_to(x)
        else
            for name in pairs(ACTIONS) do
                if inside(hits[name], x, y) and not hits[name].disabled then pressed = name end
            end
        end
        render()
    elseif t.event == "up" then
        if drag == "seek" then seek_to(x, true) end
        local name = pressed
        drag, pressed = nil, nil
        if name and inside(hits[name], x, y) then ACTIONS[name]() end
        render()
    end
end

local function on_wheel(dir)
    return function()
        if inside(hits.seek, mouse.x, mouse.y) then
            mp.commandv("seek", tostring(5 * dir), "relative+exact")
        else
            mp.commandv("add", "volume", tostring(5 * dir))
        end
    end
end

-- The bar's mouse buttons take over only while the pointer is on it (or a
-- drag that began there goes on); the pointer stays in sight there.
local function set_bindings(on)
    if on == bindings_on then return end
    bindings_on = on
    if on then
        mp.add_forced_key_binding("MBTN_LEFT", "bar_click", on_click, { complex = true })
        mp.add_forced_key_binding("MBTN_LEFT_DBL", "bar_dbl", function() end)
        -- A right-click on A-B turns the loop off; anywhere else, the menu.
        mp.add_forced_key_binding("MBTN_RIGHT", "bar_right", function()
            if inside(hits.ab, mouse.x, mouse.y) then
                mp.command('set ab-loop-a no; set ab-loop-b no; show-text "A-B loop off"')
            else
                mp.command("script-binding ferro_menu/menu")
            end
        end)
        mp.add_forced_key_binding("WHEEL_UP", "bar_wheel_up", on_wheel(1))
        mp.add_forced_key_binding("WHEEL_DOWN", "bar_wheel_down", on_wheel(-1))
        saved_autohide = mp.get_property("cursor-autohide")
        mp.set_property("cursor-autohide", "no")
    else
        for _, n in ipairs({ "bar_click", "bar_dbl", "bar_right", "bar_wheel_up", "bar_wheel_down" }) do
            mp.remove_key_binding(n)
        end
        if saved_autohide then mp.set_property("cursor-autohide", saved_autohide) end
        drag, pressed = nil, nil
    end
end

---------------------------------------------------------------- showing

local function hide()
    if not shown then return end
    shown = false
    if tick_timer then tick_timer:kill() end
    if hide_timer then hide_timer:kill() end
    if tip_timer then tip_timer:kill() end
    set_bindings(false)
    if thumb_sent then
        thumb_sent = nil
        mp.commandv("script-message-to", "ferro_thumbs", "thumb-hide")
    end
    last_data = nil
    overlay:remove()
end

local function show()
    if not shown then
        shown = true
        if not mp.get_property_native("pause") then tick_timer:resume() end
    end
end

hide_timer = mp.add_timeout(HIDE_AFTER, function()
    if over_bar or drag then return end
    hide()
end, true)
tick_timer = mp.add_periodic_timer(TICK, render, true)
tip_timer = mp.add_timeout(0.6, function()
    tip_ready = true
    render()
end, true)

mp.observe_property("mouse-pos", "native", function(_, m)
    if not m then return end
    local moved = m.x ~= mouse.x or m.y ~= mouse.y
    mouse.x, mouse.y, mouse.inside = m.x, m.y, m.hover
    if not m.hover and not drag then
        -- The pointer left the window.
        over_bar = false
        hide_timer:kill()
        hide_timer.timeout = 0.3
        hide_timer:resume()
        hide_timer.timeout = HIDE_AFTER
        return
    end
    if not moved then return end
    show()
    local _, h = mp.get_osd_size()
    over_bar = h and m.y >= h - BAR_H * scale() or false
    set_bindings(over_bar or drag ~= nil)
    if drag == "seek" then
        seek_to(m.x, false)
    elseif drag == "volume" then
        volume_to(m.x)
    end
    -- A tooltip appears once the pointer rests on a button a moment.
    if tip_ready then tip_ready = false end
    tip_timer:kill()
    if over_bar then tip_timer:resume() end
    hide_timer:kill()
    if not over_bar then hide_timer:resume() end
    render()
end)

-- Things on the bar that change now and then: drawn again at once (the
-- drawing is only sent on when it came out different).
local function changed() if shown then render() end end
for _, p in ipairs({ "pause", "mute", "volume", "loop-file", "ab-loop-a", "ab-loop-b", "playlist-pos",
    "playlist-count", "fullscreen", "duration", "idle-active", "osd-dimensions" }) do
    mp.observe_property(p, "none", changed)
end
mp.register_event("seek", changed)
-- Paused, the time doesn't move: no need to look at it.
mp.observe_property("pause", "bool", function(_, p)
    if p then tick_timer:kill() elseif shown then tick_timer:resume() end
end)

-- The menu (ferro-menu.lua) hides the bar when it opens.
mp.register_script_message("osc-hide", hide)
