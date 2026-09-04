-- breadboard <-> breadd bridge.
--
-- Turns `bread.board.*` events into desktop notifications. Drop this in
-- `~/.config/bread/modules/` and `bread reload`.
--
-- The server emits these events with `bread-emit` (config: notifications.bread_events
-- = true), so this only fires when breadboard-server and breadd run on the same
-- machine. For the hosted case, use the web app's browser notifications instead.

local M = bread.module({ name = "breadboard", version = "1.0.0" })

-- Set this to your own Forgejo login to filter "assigned to you" notifications.
local ME = bread.machine.name() -- override with your login if it differs

function M.on_load()
    bread.on("bread.board.card.assigned", function(event)
        local d = event.data or {}
        bread.notify(("%s assigned"):format(d.key or "an issue"), {
            title = "breadboard",
            urgency = "normal",
            timeout = 4000,
        })
    end)

    bread.on("bread.board.cycle.started", function(event)
        local d = event.data or {}
        bread.notify(("cycle %s started (%s issues)"):format(d.name or "?", d.issues or 0), {
            title = "breadboard",
        })
    end)

    bread.on("bread.board.cycle.ended", function(event)
        local d = event.data or {}
        bread.notify(("cycle %s ended"):format(d.name or "?"), { title = "breadboard" })
    end)

    bread.on("bread.board.sync.drift", function(event)
        local d = event.data or {}
        bread.notify(("sync drift: %s (%s)"):format(d.key or "?", d.field or "?"), {
            title = "breadboard",
            urgency = "critical",
        })
    end)

    local _ = ME
end

return M
