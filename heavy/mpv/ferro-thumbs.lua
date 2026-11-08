-- ferro-thumbs: the picture over the timeline in FerroOS's Video Player.
-- The controller (osc.lua) says when and where the pointer is on the
-- timeline; this draws that moment's frame there. Each picture is made by a
-- second mpv, inside the same ferro-jail as this one (no internet, nothing
-- else may run), reading only from a keyframe so it's quick.
--
-- Starting that mpv costs about a fifth of a second of the processor
-- whatever it's asked, so pictures are made sparingly: the timeline has
-- STEPS fixed places; while the pointer sweeps along it, the nearest picture
-- already made is shown and at most one new one is made every EVERY
-- seconds; once it rests a moment (REST), the picture under it is made, then
-- the one either side. A place made once is shown at once from then on. One
-- mpv at a time, none when the pointer is off the timeline.

local mp = require "mp"
local utils = require "mp.utils"

local ID = 47            -- the overlay this draws in
local WIDTH = 200        -- the picture's width, in pixels
local GAP = 12           -- between the picture and the timeline
local STEPS = 100        -- places along the timeline with a picture
local AHEAD = 1          -- places either side made while hovering
local REST = 0.2         -- seconds the pointer rests before a picture is made
local EVERY = 0.5        -- while it moves, a new picture at most this often
local last_made = -10    -- when the last picture was begun

local dir = os.getenv("XDG_RUNTIME_DIR") or "/tmp"
local base = string.format("%s/ferro-thumb-%d", dir, utils.getpid())
local partial = base .. ".part"

local busy = false
local wanted = nil       -- {step, x, y}: the latest place asked for
local made = {}          -- step -> {w, h} (false: couldn't be made)
local shown = nil        -- the step being shown
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
local rest               -- the timer that waits for the pointer to rest

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
        -- Go on to the next only while the pointer rests (while it moves,
        -- the rest timer starts making again once it stops).
        if not rest:is_enabled() then make() end
    end
end

make = function()
    if busy then return end
    -- Only while the pointer is on the timeline.
    if not wanted then return end
    local step = next_step()
    local path = mp.get_property("path")
    local duration = mp.get_property_number("duration")
    local w, h = size()
    if not step or not path or not w or not duration or duration <= 0 or path:find("://") then return end
    busy = true
    last_made = mp.get_time()
    local t = duration * (step + 0.5) / STEPS
    mp.command_native_async({
        name = "subprocess",
        playback_only = false,
        capture_stdout = false,
        capture_stderr = false,
        args = {
            "mpv", "--no-config", "--really-quiet", "--no-terminal", "--load-scripts=no",
            "--ytdl=no", "--audio=no", "--sub=no", "--hr-seek=no", "--frames=1",
            -- A small picture from a keyframe: one thread, no loop filter,
            -- nothing read ahead (about a quarter cheaper).
            "--load-auto-profiles=no", "--input-default-bindings=no", "--cache=no",
            "--demuxer-readahead-secs=0", "--vd-lavc-threads=1", "--vd-lavc-fast",
            "--vd-lavc-skiploopfilter=all", "--vd-lavc-skipframe=nonref",
            "--start=" .. string.format("%.2f", t),
            -- A thin black frame round the picture, like Windows 95 draws.
            string.format("--vf=lavfi=[scale=%d:%d,pad=%d:%d:1:1:black,format=bgra]", w - 2, h - 2, w, h),
            "--of=rawvideo", "--ovc=rawvideo", "--o=" .. partial,
            "--", path,
        },
    }, done(step, w, h, file_id))
end

-- Once the pointer rests: the picture under it, then the ones beside it.
rest = mp.add_timeout(REST, function() make() end, true)

mp.register_script_message("thumb", function(t, x, y)
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
        for d = 1, 6 do
            local near = (made[step - d] and step - d) or (made[step + d] and step + d)
            if near then draw(near, wanted.x, wanted.y) break end
        end
        rest:kill()
        if not busy and mp.get_time() - last_made >= EVERY then make() end
        rest:resume()
    end
end)

mp.register_script_message("thumb-hide", function()
    rest:kill()
    wanted = nil
    hide()
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
