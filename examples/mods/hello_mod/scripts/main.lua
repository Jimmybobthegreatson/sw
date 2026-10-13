-- hello_mod: the smallest useful mod.
local greet = require("greeting") -- scripts/greeting.lua (mod-local modules only)

log.info("hello_mod loaded, api", mod.api_version, "reloaded:", mod.reloaded)

cvar.register("hello.loud", false, "Shout the greeting")

console.register("hello", "hello [name] - greet someone", function(args)
  local who = args[1] or "Agent 47"
  local text = greet.make(who, cvar.get("hello.loud"))
  ui.message(text, 4)
  return text
end)

events.on("on_level_load", function(ev)
  ui.message(greet.make("Agent 47", false) .. " Entering " .. ev.level, 5)
end)

-- Data-driven tuning shipped by this mod lives in data/weapons.json (merge patch).
local dmg = data.get("weapons", "silverballer.damage")
if dmg then
  log.info("silverballer damage after all patches:", dmg)
end
