-- ferro-thumbs: the picture over the timeline in FerroOS's Video Player.
-- The controller (osc.lua) says when and where the pointer is on the
-- timeline; this draws that moment's frame there. Each picture is made by a
-- second mpv, inside the same ferro-jail as this one (no internet, nothing
-- else may run), reading only from a keyframe so it's quick; one at a time,
-- always for the latest place asked.

local mp = require "mp"
local utils = require "mp.utils"

local ID = 47            -- the overlay this draws in
local WIDTH = 200        -- the picture's width, in pixels
local GAP = 12           -- between the picture and the timeline

local dir = os.getenv("XDG_RUNTIME_DIR") or "/tmp"
local final = string.format("%s/ferro-thumb-%d.bgra", dir, utils.getpid())
local partial = final .. ".part"

local busy = false
local wanted = nil       -- {t, x, y}: the latest place asked for
local made = nil         -- {t, w, h}: what's in `final`
local shown = false
local last_asked = 0

local function hide()
    if shown then
        mp.command_native({ name = "overlay-remove", id = ID })
        shown = false
    end
end

-- The picture's size for this video: WIDTH wide, its own shape.
local function size()
    local p = mp.get_property_native("video-params")
    if not p or not p.dw or p.dw == 0 then return nil end
    local h = math.floor(WIDTH * p.dh / p.dw / 2 + 0.5) * 2
    return WIDTH, math.max(2, h)
end

local function draw(x, y)
    if not made then return end
    local w, h = made.w, made.h
    local ow, oh = mp.get_osd_size()
    local left = math.max(0, math.min(x - w / 2, ow - w))
    local top = math.max(0, y - h - GAP)
    mp.command_native({ name = "overlay-add", id = ID, x = math.floor(left), y = math.floor(top),
        file = final, offset = 0, fmt = "bgra", w = w, h = h, stride = w * 4 })
    shown = true
end

local make

local function done(t, w, h)
    return function(ok)
        busy = false
        local info = utils.file_info(partial)
        if ok and info and info.size == w * h * 4 then
            os.rename(partial, final)
            made = { t = t, w = w, h = h }
            if wanted then draw(wanted.x, wanted.y) end
        end
        -- The pointer moved on meanwhile: the newest place next.
        if wanted and wanted.t ~= t then make() end
    end
end

make = function()
    if busy or not wanted then return end
    local path = mp.get_property("path")
    local w, h = size()
    if not path or not w or path:find("://") then return end
    local t = wanted.t
    if made and made.t == t then
        draw(wanted.x, wanted.y)
        return
    end
    busy = true
    mp.command_native_async({
        name = "subprocess",
        playback_only = false,
        capture_stdout = false,
        capture_stderr = false,
        args = {
            "mpv", "--no-config", "--really-quiet", "--no-terminal", "--load-scripts=no",
            "--ytdl=no", "--audio=no", "--sub=no", "--hr-seek=no", "--frames=1",
            "--start=" .. t,
            string.format("--vf=lavfi=[scale=%d:%d,format=bgra]", w, h),
            "--of=rawvideo", "--ovc=rawvideo", "--o=" .. partial,
            "--", path,
        },
    }, done(t, w, h))
end

mp.register_script_message("thumb", function(t, x, y)
    last_asked = mp.get_time()
    -- Close enough is the same picture: within a tenth of a second.
    wanted = { t = string.format("%.1f", tonumber(t) or 0), x = tonumber(x) or 0, y = tonumber(y) or 0 }
    if made and made.t == wanted.t then
        draw(wanted.x, wanted.y)
    else
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

-- Another video: its own pictures.
mp.register_event("file-loaded", function()
    made = nil
    wanted = nil
    hide()
end)

mp.register_event("shutdown", function()
    os.remove(final)
    os.remove(partial)
end)
