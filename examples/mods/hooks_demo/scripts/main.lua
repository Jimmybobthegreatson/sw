-- hooks_demo: event hooks, cancellation, timers, save data and hot-reload state.

local cfg = data.get("hooks_demo/config") or { damage_scale = 1.0, banned_items = {} }

-- mod.state survives hot reload; save.* survives restarts.
mod.state.hits = mod.state.hits or 0
local kills = save.get("kills", 0)

-- Higher priority runs first; handlers may edit ev fields.
events.on("on_damage", function(ev)
  mod.state.hits = mod.state.hits + 1
  if ev.victim == world.player() then
    return -- never scale damage taken by the player
  end
  ev.amount = ev.amount * cfg.damage_scale
end, { priority = 10 })

events.on("on_pickup", function(ev)
  for _, banned in ipairs(cfg.banned_items) do
    if ev.item == banned then
      ev.cancelled = true -- veto the pickup (returning false works too)
      ui.message("You cannot pick up " .. banned, 2)
      return
    end
  end
end)

events.on("on_kill", function(ev)
  if ev.killer == world.player() then
    kills = kills + 1
    save.set("kills", kills)
    audio.play("hooks_demo_ding", { entity = ev.killer, volume = 0.7 })
    events.emit("hooks_demo.kill_counted", { total = kills })
  end
end)

events.on("on_mod_unload", function()
  save.set("last_hits", mod.state.hits)
end)

timer.every(30, function()
  log.info("hooks_demo: hits so far", mod.state.hits, "kills", kills)
end)

console.register("kills", "kills - show the kill counter", function()
  return "kills: " .. kills .. ", damage events seen: " .. mod.state.hits
end)

if mods.is_loaded("hello_mod") then
  log.info("hello_mod is present, hooks_demo loads after it")
end
