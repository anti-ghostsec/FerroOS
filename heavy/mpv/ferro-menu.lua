-- ferro-menu: the menu of FerroOS's Video Player (right-click, or the ☰
-- button). mpv 0.40's own menu (select.lua, LGPL-2.1+) with "Stats for
-- nerds" called just "Stats", and A-B loop and Delete added.

local mp = require "mp"
local input = require "mp.input"

local function ab_label()
    local a, b = mp.get_property("ab-loop-a"), mp.get_property("ab-loop-b")
    if a == "no" then return "A-B loop: set start (A)" end
    if b == "no" then return "A-B loop: set end (B)" end
    return "A-B loop: off"
end

mp.add_key_binding(nil, "menu", function()
    local subs, audios, videos = 0, 0, 0
    for _, track in pairs(mp.get_property_native("track-list", {})) do
        if track.type == "sub" then subs = subs + 1
        elseif track.type == "audio" then audios = audios + 1
        elseif track.type == "video" then videos = videos + 1 end
    end

    local menu = {
        { ab_label(), "ab-loop", true },
        { "Delete this video...", "script-binding ferro_delete/ask", mp.get_property("path") ~= nil },
        { "Subtitles", "script-binding select/select-sid", subs > 0 },
        { "Audio tracks", "script-binding select/select-aid", audios > 1 },
        { "Video tracks", "script-binding select/select-vid", videos > 1 },
        { "Videos in this folder", "script-binding select/select-playlist", mp.get_property_native("playlist-count", 0) > 1 },
        { "Chapters", "script-binding select/select-chapter", mp.get_property("chapter") ~= nil },
        { "Audio devices", "script-binding select/select-audio-device", audios > 0 },
        { "Stats", "script-binding stats/display-page-1-toggle", true },
        { "File info", "script-binding stats/display-page-5-toggle", mp.get_property("filename") ~= nil },
        { "Keys", "script-binding stats/display-page-4-toggle", true },
    }

    local labels, commands = {}, {}
    for _, entry in ipairs(menu) do
        if entry[3] then
            labels[#labels + 1] = entry[1]
            commands[#commands + 1] = entry[2]
        end
    end

    input.select({
        prompt = "",
        items = labels,
        keep_open = true,
        submit = function(i)
            mp.command(commands[i])
            if not commands[i]:find("^script%-binding select/") then
                input.terminate()
            end
        end,
    })
end)
