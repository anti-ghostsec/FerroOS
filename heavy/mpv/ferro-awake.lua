-- ferro-awake: a video keeps the screen awake (and unlocked) while it plays,
-- but only its first time through. The Video Player plays each video round
-- and round, so without this a video left looping would keep the screen
-- from locking for ever. Once it's gone round, the screen saver and the
-- lock come as they would with nothing playing; opening another video, or
-- seeking in this one, keeps the screen awake again.

local mp = require "mp"

local last = 0
local looped = false

local function awake(yes)
    mp.set_property("stop-screensaver", yes and "yes" or "no")
end

mp.register_event("file-loaded", function()
    last, looped = 0, false
    awake(true)
end)

mp.observe_property("time-pos", "number", function(_, t)
    if t then last = t end
end)

-- A seek back to the start from the end is the loop going round (mpv seeks
-- to loop a file); one from anywhere else is the viewer's.
mp.register_event("seek", function()
    local d = mp.get_property_number("duration", 0)
    if d > 0 and last >= d - 1.5 then
        looped = true
        awake(false)
    elseif looped then
        looped = false
        awake(true)
    end
end)
