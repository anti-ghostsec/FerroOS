-- ferro-thumbs: the picture over the timeline in FerroOS's Video Player.
-- The controller (osc.lua) says when and where the pointer is on the
-- timeline; this draws that moment's frame there. Each picture is made by a
-- second mpv, inside the same ferro-jail as this one (no internet, nothing
-- else may run), reading only from a keyframe so it's quick.
--
-- Starting that mpv costs about a third of a second whatever it's asked, so
-- the timeline has STEPS fixed places: the one under the pointer is made
-- first, then (while the pointer is on the timeline) those around it, and a
-- place made once is shown at once from then on.

local mp = require "mp"
local utils = require "mp.utils"

local ID = 47            -- the overlay this draws in
local WIDTH = 200        -- the picture's width, in pixels
local GAP = 12           -- between the picture and the timeline
local STEPS = 100        -- places along the timeline with a picture
local AHEAD = 12         -- places either side made while hovering

local dir = os.getenv("XDG_RUNTIME_DIR") or "/tmp"
local base = string.format("%s/ferro-thumb-%d", dir, utils.getpid())
local partial = base .. ".part"

local busy = false
local wanted = nil       -- {step, x, y}: the latest place asked for
local made = {}          -- step -> {w, h} (false: couldn't be made)
local shown = nil        -- the step being shown
local last_asked = 0
local file_id = 0        -- which video the pictures are of

local function file_of(step) return string.format("%s-%d.bgra", base, step) end

local function hide()
    if shown then
        mp.command_native({ name = "overlay-remove", id = ID })
        shown = nil
    end
end

-- The picture's size for this video: WIDTH wide, its own shape.
local function size()
    local p = mp.get_property_native("video-params")
    if not p or not p.dw or p.dw == 0 then return nil end
    local h = math.floor(WIDTH * p.dh / p.dw / 2 + 0.5) * 2
    return WIDTH, math.max(2, h)
end

local function draw(step, x, y)
    local m = made[step]
    if not m then return end
    local ow = mp.get_osd_size()
    local left = math.max(0, math.min(x - m.w / 2, ow - m.w))
    local top = math.max(0, y - m.h - GAP)
    mp.command_native({ name = "overlay-add", id = ID, x = math.floor(left), y = math.floor(top),
        file = file_of(step), offset = 0, fmt = "bgra", w = m.w, h = m.h, stride = m.w * 4 })
    shown = step
end

-- The next place to make: the one asked for, then outward from it.
local function next_step()
    local s = wanted.step
    for d = 0, AHEAD do
        for _, c in ipairs({ s + d, s - d }) do
            if c >= 0 and c < STEPS and made[c] == nil then return c end
        end
    end
    return nil
end

local make

local function done(step, w, h, id)
    return function(ok)
        busy = false
        if id == file_id then
            local info = utils.file_info(partial)
            if ok and info and info.size == w * h * 4 then
                os.rename(partial, file_of(step))
                made[step] = { w = w, h = h }
                if wanted and wanted.step == step then draw(step, wanted.x, wanted.y) end
            else
                made[step] = false
            end
        end
        make()
    end
end

make = function()
    if busy then return end
    -- Only while the pointer is on the timeline.
    if not wanted or mp.get_time() - last_asked > 1.5 then return end
    local step = next_step()
    local path = mp.get_property("path")
    local duration = mp.get_property_number("duration")
    local w, h = size()
    if not step or not path or not w or not duration or duration <= 0 or path:find("://") then return end
    busy = true
    local t = duration * (step + 0.5) / STEPS
    mp.command_native_async({
        name = "subprocess",
        playback_only = false,
        capture_stdout = false,
        capture_stderr = false,
        args = {
            "mpv", "--no-config", "--really-quiet", "--no-terminal", "--load-scripts=no",
            "--ytdl=no", "--audio=no", "--sub=no", "--hr-seek=no", "--frames=1",
            "--start=" .. string.format("%.2f", t),
            string.format("--vf=lavfi=[scale=%d:%d,format=bgra]", w, h),
            "--of=rawvideo", "--ovc=rawvideo", "--o=" .. partial,
            "--", path,
        },
    }, done(step, w, h, file_id))
end

mp.register_script_message("thumb", function(t, x, y)
    last_asked = mp.get_time()
    local duration = mp.get_property_number("duration") or 0
    local step = 0
    if duration > 0 then
        step = math.max(0, math.min(STEPS - 1, math.floor((tonumber(t) or 0) / duration * STEPS)))
    end
    wanted = { step = step, x = tonumber(x) or 0, y = tonumber(y) or 0 }
    if made[step] then
        draw(step, wanted.x, wanted.y)
    else
        -- The nearest one made, meanwhile, rather than none.
        for d = 1, 3 do
            local near = (made[step - d] and step - d) or (made[step + d] and step + d)
            if near then draw(near, wanted.x, wanted.y) break end
        end
        make()
    end
end)

mp.register_script_message("thumb-hide", function()
    wanted = nil
    hide()
end)

-- Not heard of for a while (the controller hid): no picture.
mp.add_periodic_timer(0.5, function()
    if shown and mp.get_time() - last_asked > 1.5 then
        wanted = nil
        hide()
    end
end)

local function forget()
    for step, m in pairs(made) do
        if m then os.remove(file_of(step)) end
    end
    made = {}
end

-- Another video: its own pictures.
mp.register_event("file-loaded", function()
    file_id = file_id + 1
    forget()
    wanted = nil
    hide()
end)

mp.register_event("shutdown", function()
    forget()
    os.remove(partial)
end)
