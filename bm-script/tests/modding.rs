use bm_script::events::ErrorKind;
use bm_script::package::{self, ModConfig, ModDirs, ModManager, ProblemKind};
use bm_script::*;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use tempfile::TempDir;

// ---------------------------------------------------------------- helpers

fn mk_mod(root: &Path, id: &str, extra_toml: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = root.join(id);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("mod.toml"),
        format!("id = \"{id}\"\nname = \"{id}\"\nversion = \"1.0.0\"\n{extra_toml}\n"),
    )
    .unwrap();
    for (rel, content) in files {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }
    dir
}

fn script_mod(root: &Path, id: &str, perms: &str, main_lua: &str) -> PathBuf {
    mk_mod(
        root,
        id,
        &format!("permissions = [{perms}]\n[scripts]\nentry = [\"scripts/main.lua\"]\n"),
        &[("scripts/main.lua", main_lua)],
    )
}

struct Env {
    _tmp: TempDir,
    mods_root: PathBuf,
    dirs: ModDirs,
}

fn env() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let mods_root = tmp.path().join("Mods");
    fs::create_dir_all(&mods_root).unwrap();
    let dirs = ModDirs::with_state_dir(vec![mods_root.clone()], &tmp.path().join("state"));
    Env { _tmp: tmp, mods_root, dirs }
}

fn boot(env: &Env, cfg: RuntimeConfig) -> (ScriptRuntime, Rc<MockHost>, Vec<LoadReport>) {
    let host = Rc::new(MockHost::new());
    let mut rt = ScriptRuntime::new(host.clone(), cfg);
    let mgr = ModManager::new(env.dirs.clone()).unwrap();
    let set = mgr.resolve();
    let (_, reports) = rt.load_mods(&set);
    (rt, host, reports)
}

fn examples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/mods")
}

// ---------------------------------------------------------------- manifest / packaging

#[test]
fn example_mods_are_valid_and_ordered() {
    let tmp = tempfile::tempdir().unwrap();
    let dirs = ModDirs::with_state_dir(vec![examples_dir()], tmp.path());
    let mgr = ModManager::new(dirs).unwrap();
    assert!(mgr.discovery.failures.is_empty(), "{:?}", mgr.discovery.failures);
    let set = mgr.resolve();
    assert!(!set.has_errors(), "{:?}", set.problems);
    assert_eq!(set.ids(), vec!["hello_mod", "hooks_demo"]);
    for m in mgr.mods() {
        assert!(!m.has_errors(), "{}: {:?}", m.manifest.id, m.diagnostics);
    }
}

#[test]
fn dependency_cycle_is_reported_and_mods_disabled() {
    let e = env();
    mk_mod(&e.mods_root, "a", "[dependencies]\nb = \"*\"\n", &[]);
    mk_mod(&e.mods_root, "b", "[dependencies]\nc = \"*\"\n", &[]);
    mk_mod(&e.mods_root, "c", "[dependencies]\na = \"*\"\n", &[]);
    mk_mod(&e.mods_root, "ok", "", &[]);
    mk_mod(&e.mods_root, "downstream", "[dependencies]\na = \"*\"\n", &[]);
    let set = ModManager::new(e.dirs.clone()).unwrap().resolve();
    assert_eq!(set.ids(), vec!["ok"]);
    let cyc = set.problems.iter().find(|p| p.kind == ProblemKind::Cycle).expect("cycle reported");
    assert!(cyc.message.contains("a") && cyc.message.contains("b") && cyc.message.contains("c"), "{}", cyc.message);
    assert!(set.problems.iter().any(|p| p.kind == ProblemKind::MissingDependency && p.mod_id.as_deref() == Some("downstream")));
}

#[test]
fn load_after_cycle_detected() {
    let e = env();
    mk_mod(&e.mods_root, "x", "load_after = [\"y\"]", &[]);
    mk_mod(&e.mods_root, "y", "load_after = [\"x\"]", &[]);
    let set = ModManager::new(e.dirs.clone()).unwrap().resolve();
    assert!(set.order.is_empty());
    assert!(set.problems.iter().any(|p| p.kind == ProblemKind::Cycle));
}

#[test]
fn topo_order_priority_and_dependencies() {
    let e = env();
    mk_mod(&e.mods_root, "lib", "priority = 50", &[]);
    mk_mod(&e.mods_root, "app", "priority = -5\n[dependencies]\nlib = \">=1\"\n", &[]); // must still come after lib
    mk_mod(&e.mods_root, "low", "priority = -1", &[]);
    mk_mod(&e.mods_root, "high", "priority = 5", &[]);
    mk_mod(&e.mods_root, "first", "load_before = [\"low\"]\npriority = 100", &[]);
    let set = ModManager::new(e.dirs.clone()).unwrap().resolve();
    let ids = set.ids();
    let pos = |s: &str| ids.iter().position(|x| *x == s).unwrap();
    assert!(pos("lib") < pos("app"));
    assert!(pos("first") < pos("low"));
    assert!(!set.has_errors(), "{:?}", set.problems);
}

#[test]
fn missing_dependency_version_conflict_api_and_user_disable() {
    let e = env();
    mk_mod(&e.mods_root, "needs_missing", "[dependencies]\nghost = \"*\"\n", &[]);
    mk_mod(&e.mods_root, "base", "", &[]);
    mk_mod(&e.mods_root, "wants_new", "[dependencies]\nbase = \">=2.0\"\n", &[]);
    mk_mod(&e.mods_root, "future_api", "api_version = \"^2\"", &[]);
    mk_mod(&e.mods_root, "bad_id_ok", "", &[]);
    mk_mod(&e.mods_root, "alpha", "conflicts = [\"beta\"]\npriority = 1", &[]);
    mk_mod(&e.mods_root, "beta", "", &[]);
    mk_mod(&e.mods_root, "off", "", &[]);
    let mut mgr = ModManager::new(e.dirs.clone()).unwrap();
    mgr.set_enabled("off", false).unwrap();
    let mgr = ModManager::new(e.dirs.clone()).unwrap(); // reload config from disk
    let set = mgr.resolve();
    let kinds: Vec<_> = set.problems.iter().map(|p| (p.kind, p.mod_id.clone().unwrap_or_default())).collect();
    assert!(kinds.contains(&(ProblemKind::MissingDependency, "needs_missing".into())));
    assert!(kinds.contains(&(ProblemKind::DependencyVersion, "wants_new".into())));
    assert!(kinds.contains(&(ProblemKind::Invalid, "future_api".into())));
    assert!(kinds.contains(&(ProblemKind::Conflict, "beta".into())), "{kinds:?}");
    assert!(kinds.contains(&(ProblemKind::DisabledByUser, "off".into())));
    let ids = set.ids();
    assert!(ids.contains(&"alpha") && !ids.contains(&"beta") && !ids.contains(&"off"));
}

#[test]
fn duplicate_ids_keep_highest_version() {
    let tmp = tempfile::tempdir().unwrap();
    let (r1, r2) = (tmp.path().join("r1"), tmp.path().join("r2"));
    mk_mod(&r1, "dup", "", &[]);
    let d = mk_mod(&r2, "dup", "", &[]);
    fs::write(d.join("mod.toml"), "id=\"dup\"\nname=\"d\"\nversion=\"2.0.0\"\n").unwrap();
    let dirs = ModDirs::with_state_dir(vec![r1, r2], &tmp.path().join("s"));
    let mgr = ModManager::new(dirs).unwrap();
    let set = mgr.resolve();
    assert_eq!(set.order.len(), 1);
    assert_eq!(set.order[0].manifest.version, "2.0.0");
    assert!(set.problems.iter().any(|p| p.kind == ProblemKind::DuplicateId));
}

#[test]
fn asset_conflicts_and_vfs_mount_order() {
    let e = env();
    mk_mod(&e.mods_root, "m1", "", &[("assets/Textures/Foo.TEX", "from m1"), ("assets/only1.txt", "1")]);
    mk_mod(&e.mods_root, "m2", "priority = 1\n", &[("assets/textures/foo.tex", "from m2"), ("assets/only2.txt", "2")]);
    let game = tempfile::tempdir().unwrap();
    fs::write(game.path().join("Base.txt"), "base").unwrap();
    fs::create_dir_all(game.path().join("textures")).unwrap();
    fs::write(game.path().join("textures/foo.tex"), "game").unwrap();
    let mgr = ModManager::new(e.dirs.clone()).unwrap();
    let set = mgr.resolve();
    assert_eq!(set.ids(), vec!["m1", "m2"]);
    let mut vfs = bm_vfs::Vfs::new(game.path()).unwrap();
    let rep = mgr.mount(&mut vfs, &set).unwrap();
    assert_eq!(rep.mounted, vec!["m1", "m2"]);
    assert_eq!(rep.conflicts.conflicts.len(), 1);
    let c = &rep.conflicts.conflicts[0];
    assert_eq!(c.path, "textures/foo.tex");
    assert_eq!(c.winner, "m2");
    assert_eq!(c.overrides_game, Some(true));
    assert_eq!(vfs.open("Textures/FOO.tex").unwrap(), b"from m2");
    assert_eq!(vfs.open("only1.txt").unwrap(), b"1");
    assert_eq!(vfs.open("Base.txt").unwrap(), b"base");
    assert!(rep.conflicts.game_overrides.contains_key("m1"));
}

fn write_zip(path: &Path, entries: &[(&str, &str)]) {
    use std::io::Write;
    let f = fs::File::create(path).unwrap();
    let mut z = zip::ZipWriter::new(f);
    let opts = zip::write::SimpleFileOptions::default();
    for (n, c) in entries {
        z.start_file(*n, opts).unwrap();
        z.write_all(c.as_bytes()).unwrap();
    }
    z.finish().unwrap();
}

#[test]
fn zip_mods_are_extracted_and_zip_slip_rejected() {
    let e = env();
    write_zip(
        &e.mods_root.join("zipped.zip"),
        &[
            ("mod.toml", "id=\"zipped\"\nname=\"Z\"\nversion=\"0.1.0\"\n"),
            ("assets/a.txt", "hello"),
        ],
    );
    write_zip(
        &e.mods_root.join("nested.zip"),
        &[
            ("nested_top/mod.toml", "id=\"nested\"\nname=\"N\"\nversion=\"0.1.0\"\n"),
            ("nested_top/assets/b.txt", "b"),
        ],
    );
    write_zip(
        &e.mods_root.join("evil.zip"),
        &[("mod.toml", "id=\"evil\"\nname=\"E\"\nversion=\"0.1.0\"\n"), ("../escape.txt", "boom")],
    );
    let mgr = ModManager::new(e.dirs.clone()).unwrap();
    let ids: Vec<_> = mgr.mods().iter().map(|m| m.manifest.id.as_str()).collect();
    assert!(ids.contains(&"zipped") && ids.contains(&"nested"), "{ids:?}");
    assert!(!ids.contains(&"evil"));
    assert_eq!(mgr.discovery.failures.len(), 1);
    assert!(!e.dirs.cache_dir.parent().unwrap().join("escape.txt").exists());
    let set = mgr.resolve();
    let z = set.get("zipped").unwrap();
    assert_eq!(fs::read_to_string(z.assets_root.join("a.txt")).unwrap(), "hello");
}

#[test]
fn validate_reports_manifest_problems() {
    let e = env();
    mk_mod(
        &e.mods_root,
        "broken",
        "permissions = [\"warp_drive\"]\n[scripts]\nentry = [\"scripts/missing.lua\", \"../x.lua\"]\n",
        &[],
    );
    let m = package::load_mod(&e.mods_root.join("broken"), &e.dirs.cache_dir).unwrap();
    let msgs: Vec<_> = m.diagnostics.iter().map(|d| d.message.clone()).collect();
    assert!(msgs.iter().any(|m| m.contains("warp_drive")));
    assert!(msgs.iter().any(|m| m.contains("not found")));
    assert!(msgs.iter().any(|m| m.contains("inside the mod")));
    assert!(m.has_errors());
}

#[test]
fn cli_commands() {
    let e = env();
    mk_mod(&e.mods_root, "one", "", &[("assets/a.txt", "1")]);
    mk_mod(&e.mods_root, "two", "", &[("assets/a.txt", "2")]);
    let run = |a: &[&str]| cli_run(&e.dirs, a);
    assert!(run(&["list"]).unwrap().contains("one"));
    assert!(run(&["conflicts"]).unwrap().contains("a.txt"));
    run(&["disable", "two"]).unwrap();
    assert!(run(&["list"]).unwrap().contains("disabled"));
    assert!(run(&["conflicts"]).unwrap().contains("no asset conflicts"));
    run(&["enable", "two"]).unwrap();
    assert!(run(&["order"]).unwrap().contains("two"));
    assert!(run(&["validate"]).is_ok());
    assert!(run(&["bogus"]).is_err());
    assert!(ModConfig::load(&e.dirs.config_file).unwrap().disabled.is_empty());
}

fn cli_run(dirs: &ModDirs, a: &[&str]) -> Result<String, package::ModError> {
    bm_script::cli::run_mods_command(dirs, a, None)
}

// ---------------------------------------------------------------- data patches

#[test]
fn data_patches_merge_validate_and_replace() {
    let e = env();
    mk_mod(
        &e.mods_root,
        "tune_a",
        "",
        &[
            ("data/weapons.json", r#"{"ak":{"damage":30,"mag":null},"new":{"damage":5}}"#),
            ("data/npc/guard.toml", "hp = 80\n"),
            ("data/extra.ron", "(speed: 2.5, tags: [\"a\"])"),
        ],
    );
    mk_mod(
        &e.mods_root,
        "tune_b",
        "priority = 1",
        &[
            ("data/weapons.json", r#"{"ak":{"damage":"lots"}}"#), // rejected by schema
            ("data/npc/guard.toml", "hp = 90\n"),
            ("data/extra.ron", "(tags: [\"b\"])"),
            ("data/fresh.json", r#"{"$replace": true, "only": 1}"#),
            ("data/fresh.schema.json", r#"{"type":"object"}"#),
        ],
    );
    let mgr = ModManager::new(e.dirs.clone()).unwrap();
    let set = mgr.resolve();
    let mut store = DataStore::new();
    store.set_base("weapons", json!({"ak": {"damage": 20, "mag": 30}, "pistol": {"damage": 10}}));
    store.set_base("npc/guard", json!({"hp": 50, "armor": 1}));
    store.set_schema("weapons", json!({"type":"object","additionalProperties":{"type":"object","properties":{"damage":{"type":"number","minimum":0}}}}));
    let rep = store.apply_mods(&set);
    assert_eq!(store.get("weapons").unwrap(), &json!({"ak":{"damage":30},"pistol":{"damage":10},"new":{"damage":5}}));
    assert_eq!(store.get("npc/guard").unwrap(), &json!({"hp": 90, "armor": 1}));
    assert_eq!(store.get("extra").unwrap(), &json!({"speed": 2.5, "tags": ["b"]}));
    assert_eq!(store.get("fresh").unwrap(), &json!({"only": 1}));
    assert_eq!(store.get_path("weapons", "ak.damage").unwrap(), &json!(30));
    assert_eq!(rep.errors.len(), 1, "{:?}", rep.errors);
    assert!(rep.errors[0].message.contains("schema") && rep.errors[0].mod_id == "tune_b");
    assert!(store.schema("fresh").is_some());
}

// ---------------------------------------------------------------- runtime: behaviour

#[test]
fn example_mods_run_against_mock_host() {
    let tmp = tempfile::tempdir().unwrap();
    let dirs = ModDirs::with_state_dir(vec![examples_dir()], tmp.path());
    let mgr = ModManager::new(dirs).unwrap();
    let set = mgr.resolve();
    let host = Rc::new(MockHost::new());
    let player = host.add_entity("Agent47", "player", Vec3::ZERO);
    host.set_player(player);
    let mut rt = ScriptRuntime::new(
        host.clone(),
        RuntimeConfig { save_dir: Some(tmp.path().join("saves")), ..Default::default() },
    );
    rt.data_mut().set_base("weapons", json!({"silverballer": {"damage": 25}}));
    let (drep, reports) = rt.load_mods(&set);
    assert!(drep.errors.is_empty(), "{:?}", drep.errors);
    assert!(reports.iter().all(|r| r.ok), "{reports:?}");
    assert!(host.log_text().contains("silverballer damage after all patches: 40"), "{}", host.log_text());

    // on_level_load -> ui message
    rt.dispatch(&Event::level_load("M01/M01_main"));
    assert!(host.calls().iter().any(|c| matches!(c, HostCall::Message(t, _) if t.contains("M01/M01_main"))));

    // damage scaling for NPCs, not for the player
    let npc = host.add_entity("Guard", "npc", Vec3::ZERO);
    let out = rt.dispatch(&Event::damage(npc.0, Some(player.0), 10.0, Some("Silverballer"), None));
    assert_eq!(out.payload["amount"].as_f64().unwrap(), 15.0);
    let out = rt.dispatch(&Event::damage(player.0, Some(npc.0), 10.0, None, None));
    assert_eq!(out.payload["amount"].as_f64().unwrap(), 10.0);

    // pickup veto
    let out = rt.dispatch(&Event::pickup(player.0, "Crowbar", None));
    assert!(out.cancelled);
    assert!(!rt.dispatch(&Event::pickup(player.0, "Coin", None)).cancelled);

    // kill -> save + custom event + sound
    rt.dispatch(&Event::kill(npc.0, Some(player.0), Some("Garrote")));
    assert_eq!(rt.mod_save("hooks_demo").unwrap()["kills"], json!(1));
    assert!(host.calls().iter().any(|c| matches!(c, HostCall::PlaySound(n, _) if n == "hooks_demo_ding")));

    // console commands & cvars
    assert_eq!(rt.run_console("kills").unwrap().unwrap(), "kills: 1, damage events seen: 2");
    assert_eq!(rt.run_console("hello Diana").unwrap().unwrap(), "Hello, Diana.");
    assert!(rt.run_console("hello.loud true").unwrap().is_ok());
    assert_eq!(rt.run_console("hello Diana").unwrap().unwrap(), "HELLO, DIANA.!");
    assert!(rt.run_console("nonexistent").is_none());

    rt.shutdown();
    let saved = fs::read_to_string(tmp.path().join("saves/hooks_demo.json")).unwrap();
    assert!(saved.contains("\"kills\": 1"), "{saved}");
}

#[test]
fn event_priority_cancel_once_and_error_isolation() {
    let e = env();
    script_mod(
        &e.mods_root,
        "a",
        "",
        r#"
        events.on("on_damage", function(ev) ev.amount = ev.amount + 1; log.info("a-low") end, { priority = -5 })
        events.on("on_damage", function(ev) log.info("a-high"); ev.amount = ev.amount * 10 end, { priority = 5 })
        events.on("on_damage", function(ev) error("boom") end, { priority = 4 })
        events.on("on_pickup", function(ev) ev.cancelled = true end, { priority = 1 })
        events.on("on_pickup", function(ev) log.info("saw cancelled=" .. tostring(ev.cancelled)) end, { run_if_cancelled = true })
        events.on("on_pickup", function(ev) log.info("should not run") end)
        events.on("on_level_load", function(ev) log.info("once!") end, { once = true })
        local h = events.on("on_level_unload", function() log.info("unsub failed") end)
        events.off(h)
        events.on("on_kill", function() return false end) -- not cancelable: ignored
        "#,
    );
    script_mod(&e.mods_root, "b", "", r#"events.on("on_damage", function(ev) log.info("b-mid"); ev.extra = "x" end, { priority = 0 })"#);
    let (rt, host, reports) = boot(&e, Default::default());
    assert!(reports.iter().all(|r| r.ok));
    let out = rt.dispatch(&Event::damage(1, None, 1.0, None, None));
    assert_eq!(out.payload["amount"].as_f64().unwrap(), 11.0); // (1*10)+1
    assert_eq!(out.payload["extra"], json!("x"));
    assert_eq!(out.errors.len(), 1);
    assert_eq!(out.errors[0].kind, ErrorKind::Runtime);
    let log = host.log_text();
    let i = |s: &str| log.find(s).unwrap_or_else(|| panic!("missing {s} in {log}"));
    assert!(i("a-high") < i("b-mid") && i("b-mid") < i("a-low"));

    let out = rt.dispatch(&Event::pickup(1, "x", None));
    assert!(out.cancelled);
    let log = host.log_text();
    assert!(log.contains("saw cancelled=true") && !log.contains("should not run"));

    rt.dispatch(&Event::level_load("l"));
    rt.dispatch(&Event::level_load("l"));
    assert_eq!(host.log_text().matches("once!").count(), 1);
    assert!(!rt.dispatch(&Event::kill(1, None, None)).cancelled);
    rt.dispatch(&Event::new("on_level_unload", json!({"level": "x"})));
    assert!(!host.log_text().contains("unsub failed"));
}

#[test]
fn custom_events_between_mods_and_reserved_names() {
    let e = env();
    script_mod(
        &e.mods_root,
        "emitter",
        "",
        r#"
        events.on("on_level_load", function(ev)
          local r = events.emit("emitter.ping", { n = 1 })
          log.info("emit result", r.cancelled, r.payload.n, r.payload.pong)
        end)
        log.info("reserved:", pcall(events.emit, "on_damage", {}))
        "#,
    );
    script_mod(&e.mods_root, "listener", "", r#"events.on("emitter.ping", function(ev) ev.pong = ev.n + 1; ev.cancelled = true end)"#);
    let (rt, host, _) = boot(&e, Default::default());
    rt.dispatch(&Event::level_load("l"));
    let log = host.log_text();
    assert!(log.contains("emit result true 1 2"), "{log}");
    assert!(log.contains("reserved: false"), "{log}");
}

#[test]
fn entity_world_audio_ui_bindings() {
    let e = env();
    script_mod(
        &e.mods_root,
        "world_mod",
        r#""world", "entities.read", "entities.write", "audio", "fx", "ui", "console""#,
        r#"
        local id = entity.spawn("guard", { name = "G1", pos = vec3.new(1, 2, 3), model = "guard.prm", components = { hp = 10 } })
        local info = entity.info(id)
        log.info("spawned", info.name, info.position.x, info.position.y, info.model, entity.get_component(id, "hp"))
        entity.set_position(id, { 5, 6, 7 })
        entity.set_rotation(id, { x = 0, y = 90, z = 0 })
        entity.set_model(id, "other.prm")
        entity.set_component(id, "hp", { cur = 5, max = 10 })
        entity.damage(id, 3.5, nil)
        local found = entity.query({ kind = "guard", radius = 100, near = { 5, 6, 7 } })
        log.info("found", #found, entity.find("g1") == id, entity.exists(id), world.level(), world.time())
        local hit = world.raycast({ 0, 0, 0 }, { 5, 6, 7 })
        log.info("hit", hit.entity == id)
        local h = audio.play("gunshot", { pos = { 1, 1, 1 }, volume = 0.5 })
        audio.stop(h); audio.music("cue_a"); audio.music_state("alert"); audio.music_event("kill")
        fx.particles("spark", { 1, 2, 3 }, { scale = 2 })
        ui.show_screen("map"); ui.hide_screen("map"); ui.set_hud_var("ammo", 12); ui.message("hi")
        console.print("printed"); log.info(console.exec("status"))
        entity.kill(id)
        entity.despawn(id)
        "#,
    );
    let host = Rc::new(MockHost::new());
    host.set_level("M01/M01_main");
    host.set_time(12.5);
    let mut rt = ScriptRuntime::new(host.clone(), Default::default());
    let set = ModManager::new(e.dirs.clone()).unwrap().resolve();
    let (_, reports) = rt.load_mods(&set);
    assert!(reports.iter().all(|r| r.ok), "{reports:?} {}", host.log_text());
    let log = host.log_text();
    assert!(log.contains("spawned G1 1 2 guard.prm 10"), "{log}");
    assert!(log.contains("found 1 true true M01/M01_main 12.5"), "{log}");
    assert!(log.contains("hit true"), "{log}");
    assert!(log.contains("mock ran: status"), "{log}");
    let calls = host.calls();
    assert!(calls.contains(&HostCall::HudVar("ammo".into(), json!(12))));
    assert!(calls.contains(&HostCall::SetPosition(EntityId(1), Vec3::new(5.0, 6.0, 7.0))));
    assert!(calls.contains(&HostCall::SetRotation(EntityId(1), Vec3::new(0.0, 90.0, 0.0))));
    assert!(calls.contains(&HostCall::SetComponent(EntityId(1), "hp".into(), json!({"cur": 5, "max": 10}))));
    assert!(calls.contains(&HostCall::Damage(EntityId(1), 3.5, None)));
    assert!(calls.contains(&HostCall::MusicState("alert".into())));
    assert!(calls.contains(&HostCall::Despawn(EntityId(1))));
    assert!(host.entity(EntityId(1)).is_none());
}

#[test]
fn permissions_are_enforced() {
    let e = env();
    script_mod(
        &e.mods_root,
        "nope",
        "",
        r#"
        log.info("spawn:", pcall(entity.spawn, "x"))
        log.info("play:", pcall(audio.play, "x"))
        log.info("save:", pcall(save.set, "k", 1))
        local ok, err = pcall(entity.spawn, "x")
        log.info("ERR", err)
        "#,
    );
    let (_rt, host, reports) = boot(&e, Default::default());
    assert!(reports[0].ok);
    let log = host.log_text();
    assert!(log.contains("spawn: false") && log.contains("play: false") && log.contains("save: false"), "{log}");
    assert!(log.contains("permission denied") && log.contains("entities.write"), "{log}");
    assert!(!host.calls().iter().any(|c| matches!(c, HostCall::Spawn(_))));
}

#[test]
fn host_errors_become_lua_errors() {
    let e = env();
    script_mod(&e.mods_root, "he", r#""entities.write""#, r#"log.info("r:", pcall(entity.despawn, 999))"#);
    let (_rt, host, _) = boot(&e, Default::default());
    assert!(host.log_text().contains("r: false"), "{}", host.log_text());
    assert!(host.log_text().contains("host error"));
}

#[test]
fn timers_cvars_console_and_update() {
    let e = env();
    script_mod(
        &e.mods_root,
        "t",
        r#""console""#,
        r#"
        cvar.register("t.speed", 1.5, "speed")
        events.on("on_cvar_changed", function(ev) log.info("cvar", ev.name, ev.old, ev.value) end)
        timer.after(1, function() log.info("after fired") end)
        local n = 0
        local id
        id = timer.every(2, function() n = n + 1; log.info("tick", n); if n == 2 then timer.cancel(id) end end)
        events.on("on_update", function(ev) mod.state.last_dt = ev.dt end)
        console.register("add", "add a b", function(a) return tostring(tonumber(a[1]) + tonumber(a[2])) end)
        console.register("fail", "boom", function() error("cmd failed") end)
        "#,
    );
    let (rt, host, _) = boot(&e, Default::default());
    rt.update(0.5, 0.5);
    assert!(!host.log_text().contains("after fired"));
    rt.update(0.5, 1.5);
    assert!(host.log_text().contains("after fired"));
    rt.update(0.5, 2.5);
    rt.update(0.5, 4.5);
    rt.update(0.5, 6.5);
    rt.update(0.5, 8.5);
    assert_eq!(host.log_text().matches("tick").count(), 2);
    assert_eq!(rt.mod_state("t").unwrap()["last_dt"], json!(0.5));
    assert_eq!(rt.run_console("add 2 3").unwrap().unwrap(), "5");
    assert!(rt.run_console("fail").unwrap().is_err());
    assert_eq!(rt.run_console("t.speed").unwrap().unwrap(), "t.speed = 1.5");
    rt.run_console("t.speed 3").unwrap().unwrap();
    assert_eq!(rt.cvar_get("t.speed").unwrap(), json!(3));
    assert!(host.log_text().contains("cvar t.speed 1.5 3"), "{}", host.log_text());
    assert!(rt.run_console("t.speed notanumber").unwrap().is_err());
    assert!(rt.console_entries().iter().any(|(n, _)| n == "add"));
}

#[test]
fn console_command_name_collision_between_mods() {
    let e = env();
    script_mod(&e.mods_root, "first", r#""console""#, r#"console.register("dup", "", function() end)"#);
    script_mod(&e.mods_root, "second", r#""console""#, r#"console.register("dup", "", function() end)"#);
    let (_rt, _h, reports) = boot(&e, Default::default());
    assert!(reports.iter().find(|r| r.mod_id == "first").unwrap().ok);
    let second = reports.iter().find(|r| r.mod_id == "second").unwrap();
    assert!(!second.ok && second.errors[0].contains("already registered"), "{second:?}");
}

#[test]
fn require_is_confined_to_the_mod() {
    let e = env();
    mk_mod(
        &e.mods_root,
        "req",
        "[scripts]\nentry = [\"scripts/main.lua\"]\n",
        &[
            ("scripts/main.lua", r#"
                local u = require("util")
                local s = require("sub.thing")
                log.info("util", u.v, s.v, require("util") == u)
                log.info("escape", pcall(require, "../secret"))
                log.info("loop", pcall(require, "loopa"))
                log.info("txt", mod.read_text("notes.txt"), mod.read_json("conf.json").k)
                log.info("read escape", pcall(mod.read_text, "../x"))
            "#),
            ("scripts/util.lua", "return { v = 1 }"),
            ("scripts/sub/thing.lua", "return { v = 2 }"),
            ("scripts/loopa.lua", "return require('loopa')"),
            ("notes.txt", "hello notes"),
            ("conf.json", r#"{"k": "v"}"#),
            ("../secret.lua", "return 1"),
        ],
    );
    let (_rt, host, reports) = boot(&e, Default::default());
    assert!(reports[0].ok, "{reports:?}");
    let log = host.log_text();
    assert!(log.contains("util 1 2 true"), "{log}");
    assert!(log.contains("escape false"), "{log}");
    assert!(log.contains("loop false"), "{log}");
    assert!(log.contains("txt hello notes v"), "{log}");
    assert!(log.contains("read escape false"), "{log}");
}

#[test]
fn mods_are_isolated_from_each_other() {
    let e = env();
    script_mod(&e.mods_root, "iso_a", "", "leaky = 42; mod.state.x = 1");
    script_mod(&e.mods_root, "iso_b", "", r#"log.info("leaky is", tostring(leaky)); log.info("loaded", tostring(mods.is_loaded("iso_a")), table.concat(mods.list(), ","))"#);
    let (_rt, host, _) = boot(&e, Default::default());
    assert!(host.log_text().contains("leaky is nil"), "{}", host.log_text());
    assert!(host.log_text().contains("loaded true iso_a"), "{}", host.log_text());
}

// ---------------------------------------------------------------- sandbox

#[test]
fn sandbox_blocks_dangerous_globals() {
    let e = env();
    script_mod(
        &e.mods_root,
        "esc",
        "",
        r#"
        local function probe(name, v) log.info("PROBE " .. name .. " " .. tostring(v == nil)) end
        probe("os", os); probe("io", io); probe("debug", debug); probe("package", package)
        probe("loadstring", loadstring); probe("load", load); probe("dofile", dofile); probe("loadfile", loadfile)
        probe("getfenv", getfenv); probe("setfenv", setfenv); probe("newproxy", newproxy)
        log.info("PCALL require os " .. tostring(pcall(require, "os")))
        log.info("PCALL require io " .. tostring(pcall(require, "io")))
        -- cannot tamper with the shared standard library or string metatable
        log.info("PCALL string.rep=nil " .. tostring(pcall(function() string.rep = nil end)))
        log.info("PCALL string.evil " .. tostring(pcall(function() string.evil = 1 end)))
        log.info("PCALL strmt " .. tostring(pcall(function() getmetatable("").__index = {} end)))
        log.info("PCALL strmt2 " .. tostring(pcall(function() getmetatable("").evil = 1 end)))
        log.info("PCALL math " .. tostring(pcall(function() math.floor = nil end)))
        log.info("PCALL events " .. tostring(pcall(function() events.on = nil end)))
        log.info("PCALL setmt_G " .. tostring(pcall(function() setmetatable(_G, {}) end)))
        log.info("PCALL bytecode " .. tostring(pcall(function() return string.dump end)))
        local ok = pcall(function() return coroutine.wrap(function() return 1 end)() end)
        log.info("PCALL coroutine " .. tostring(ok))
        "#,
    );
    let (_rt, host, reports) = boot(&e, Default::default());
    assert!(reports[0].ok, "{reports:?}");
    let log = host.log_text();
    for n in ["os", "io", "debug", "package", "loadstring", "load", "dofile", "loadfile", "getfenv", "setfenv", "newproxy"] {
        assert!(log.contains(&format!("PROBE {n} true")), "{n} reachable!\n{log}");
    }
    for l in ["require os false", "require io false", "string.rep=nil false", "string.evil false", "strmt false", "strmt2 false", "math false", "events false"] {
        assert!(log.contains(&format!("PCALL {l}")), "{l}\n{log}");
    }
    assert!(log.contains("PCALL coroutine true"));
}

#[test]
fn infinite_loop_at_load_is_stopped() {
    let e = env();
    script_mod(&e.mods_root, "spin", "", "while true do end");
    script_mod(&e.mods_root, "fine", "", r#"log.info("fine ran")"#);
    let cfg = RuntimeConfig { limits: Limits { load_time: std::time::Duration::from_millis(100), ..Default::default() }, ..Default::default() };
    let t = std::time::Instant::now();
    let (rt, host, reports) = boot(&e, cfg);
    assert!(t.elapsed().as_secs() < 5);
    let spin = reports.iter().find(|r| r.mod_id == "spin").unwrap();
    assert!(!spin.ok && spin.errors[0].contains("time budget"), "{spin:?}");
    assert!(host.log_text().contains("fine ran"));
    assert!(rt.mod_ids().contains("fine") && !rt.mod_ids().contains("spin"));
}

#[test]
fn infinite_loop_in_handler_times_out_and_disables_mod() {
    let e = env();
    script_mod(&e.mods_root, "hang", "", r#"events.on("on_update", function() while true do end end)"#);
    script_mod(&e.mods_root, "ok", "", r#"events.on("on_update", function() mod.state.n = (mod.state.n or 0) + 1 end)"#);
    // pcall cannot be used to swallow the timeout forever
    script_mod(&e.mods_root, "sneaky", "", r#"events.on("on_update", function() pcall(function() while true do end end) mod.state.survived = true end)"#);
    let cfg = RuntimeConfig { limits: Limits { call_time: std::time::Duration::from_millis(20), ..Default::default() }, ..Default::default() };
    let (rt, _host, _) = boot(&e, cfg);
    for i in 0..5 {
        rt.update(0.016, i as f64);
    }
    let st = rt.mod_status();
    let hang = st.iter().find(|s| s.id == "hang").unwrap();
    assert!(hang.faulted && hang.strikes >= 3, "{hang:?}");
    let ok = st.iter().find(|s| s.id == "ok").unwrap();
    assert!(!ok.faulted);
    assert_eq!(rt.mod_state("ok").unwrap()["n"], json!(5));
    let errs = rt.take_errors();
    assert!(errs.iter().any(|e| e.kind == ErrorKind::Timeout && e.mod_id == "hang"));
}

#[test]
fn memory_bomb_is_capped() {
    let e = env();
    script_mod(
        &e.mods_root,
        "bomb",
        "",
        r#"
        events.on("on_update", function()
          local t = {}
          for i = 1, 1e9 do t[i] = string.rep("x", 100000) .. i end
        end)
        events.on("on_level_load", function() local s = string.rep("x", 100000000) end)
        "#,
    );
    script_mod(&e.mods_root, "bomb_load", "", r#"local t = {} while true do t[#t + 1] = string.rep("y", 65536) .. #t end"#);
    let cfg = RuntimeConfig {
        limits: Limits {
            memory_bytes: 16 << 20,
            call_time: std::time::Duration::from_secs(10),
            load_time: std::time::Duration::from_secs(10),
            ..Default::default()
        },
        ..Default::default()
    };
    let (rt, host, reports) = boot(&e, cfg);
    let bl = reports.iter().find(|r| r.mod_id == "bomb_load").unwrap();
    assert!(!bl.ok && bl.errors[0].to_lowercase().contains("memory"), "{bl:?}");
    rt.update(0.016, 1.0);
    let errs = rt.take_errors();
    assert!(errs.iter().any(|e| e.kind == ErrorKind::Memory && e.mod_id == "bomb"), "{errs:?}");
    let out = rt.dispatch(&Event::level_load("x"));
    assert!(!out.errors.is_empty());
    // the host process is still alive and healthy
    assert!(host.log_text().to_lowercase().contains("memory"));
}

#[test]
fn stack_overflow_and_syntax_errors_are_contained() {
    let e = env();
    script_mod(&e.mods_root, "rec", "", r#"local function f() return 1 + f() end events.on("on_update", function() f() end)"#);
    script_mod(&e.mods_root, "syn", "", "this is not lua !!!");
    let (rt, _h, reports) = boot(&e, Default::default());
    assert!(!reports.iter().find(|r| r.mod_id == "syn").unwrap().ok);
    rt.update(0.016, 1.0);
    assert!(rt.take_errors().iter().any(|e| e.mod_id == "rec"));
}

#[test]
fn bytecode_chunks_are_rejected() {
    let e = env();
    script_mod(&e.mods_root, "bc", "", "\u{1b}Lua binary junk");
    let (_rt, _h, reports) = boot(&e, Default::default());
    assert!(!reports[0].ok);
}

// ---------------------------------------------------------------- hot reload

fn touch_write(p: &Path, content: &str) {
    // Make sure size or mtime changes even on coarse timestamp filesystems.
    std::thread::sleep(std::time::Duration::from_millis(30));
    fs::write(p, content).unwrap();
}

#[test]
fn hot_reload_preserves_state_and_recovers_from_errors() {
    let e = env();
    let dir = script_mod(
        &e.mods_root,
        "hot",
        r#""console""#,
        r#"
        mod.state.count = (mod.state.count or 0) + 1
        cvar.register("hot.v", 1)
        log.info("v1 loaded count=" .. mod.state.count .. " reloaded=" .. tostring(mod.reloaded))
        events.on("on_update", function() mod.state.ticks = (mod.state.ticks or 0) + 1 end)
        events.on("on_level_load", function() log.info("handler v1") end)
        "#,
    );
    let (mut rt, host, _) = boot(&e, Default::default());
    rt.update(0.1, 1.0);
    rt.update(0.1, 2.0);
    rt.cvar_set("hot.v", json!(7)).unwrap();
    assert!(rt.poll_hot_reload().is_empty());

    let main = dir.join("scripts/main.lua");
    touch_write(
        &main,
        r#"
        mod.state.count = (mod.state.count or 0) + 1
        cvar.register("hot.v", 1)
        log.info("v2 loaded count=" .. mod.state.count .. " reloaded=" .. tostring(mod.reloaded) .. " ticks=" .. tostring(mod.state.ticks))
        events.on("on_level_load", function() log.info("handler v2") end)
        "#,
    );
    let r = rt.poll_hot_reload();
    assert_eq!(r.len(), 1);
    assert!(r[0].ok, "{r:?}");
    let log = host.log_text();
    assert!(log.contains("v2 loaded count=2 reloaded=true ticks=2"), "{log}");
    // old handlers are gone, new ones active, no duplicates
    rt.dispatch(&Event::level_load("x"));
    let log = host.log_text();
    assert!(log.contains("handler v2") && !log.contains("handler v1"), "{log}");
    // cvar value survived the reload
    assert_eq!(rt.cvar_get("hot.v").unwrap(), json!(7));
    assert!(rt.poll_hot_reload().is_empty());

    // Break the script: reload fails, mod unloaded, state kept for later.
    touch_write(&main, "this is broken (((");
    let r = rt.poll_hot_reload();
    assert_eq!(r.len(), 1);
    assert!(!r[0].ok);
    assert!(!rt.mod_ids().contains("hot"));
    assert!(rt.poll_hot_reload().is_empty());

    // Fix it: recovers with the earlier state.
    touch_write(&main, r#"mod.state.count = (mod.state.count or 0) + 1; log.info("v3 count=" .. mod.state.count)"#);
    let r = rt.poll_hot_reload();
    assert!(r.len() == 1 && r[0].ok, "{r:?}");
    assert!(host.log_text().contains("v3 count=3"), "{}", host.log_text());
    assert!(rt.mod_ids().contains("hot"));
}

#[test]
fn hot_reload_of_required_module_and_data() {
    let e = env();
    let dir = mk_mod(
        &e.mods_root,
        "hotdata",
        "[scripts]\nentry = [\"scripts/main.lua\"]\n",
        &[
            ("scripts/main.lua", r#"local m = require("m"); log.info("m=" .. m.v .. " d=" .. tostring(data.get("cfg", "x")))
               events.on("on_data_reload", function() log.info("data reloaded x=" .. tostring(data.get("cfg", "x"))) end)"#),
            ("scripts/m.lua", "return { v = 1 }"),
            ("data/cfg.json", r#"{"x": 1}"#),
        ],
    );
    let (mut rt, host, _) = boot(&e, Default::default());
    assert!(host.log_text().contains("m=1 d=1"));
    touch_write(&dir.join("scripts/m.lua"), "return { v = 22 }");
    let r = rt.poll_hot_reload();
    assert!(r.iter().all(|x| x.ok) && !r.is_empty());
    assert!(host.log_text().contains("m=22"), "{}", host.log_text());
    touch_write(&dir.join("data/cfg.json"), r#"{"x": 5}"#);
    let r = rt.poll_hot_reload();
    assert!(r.iter().any(|x| x.kind == ReloadKind::Data && x.ok), "{r:?}");
    assert!(host.log_text().contains("data reloaded x=5"), "{}", host.log_text());
}

#[test]
fn save_data_persists_across_runtimes_and_respects_size_cap() {
    let e = env();
    script_mod(
        &e.mods_root,
        "sv",
        r#""save""#,
        r#"
        local runs = save.get("runs", 0) + 1
        save.set("runs", runs)
        save.set("nested", { a = { 1, 2, 3 }, b = "x" })
        save.flush()
        log.info("runs " .. runs)
        "#,
    );
    let cfg = || RuntimeConfig { save_dir: Some(e.dirs.save_dir.clone()), ..Default::default() };
    let (mut rt, host, _) = boot(&e, cfg());
    assert!(host.log_text().contains("runs 1"));
    rt.shutdown();
    let (mut rt, host, _) = boot(&e, cfg());
    assert!(host.log_text().contains("runs 2"), "{}", host.log_text());
    assert_eq!(rt.mod_save("sv").unwrap()["nested"], json!({"a": [1, 2, 3], "b": "x"}));
    rt.shutdown();

    // size cap
    let e2 = env();
    script_mod(&e2.mods_root, "big", r#""save""#, r#"save.set("blob", string.rep("z", 2000)); log.info("flush", pcall(save.flush))"#);
    let cfg = RuntimeConfig {
        save_dir: Some(e2.dirs.save_dir.clone()),
        limits: Limits { max_save_bytes: 1000, ..Default::default() },
    };
    let (_rt, host, _) = boot(&e2, cfg);
    assert!(host.log_text().contains("flush false"), "{}", host.log_text());
    assert!(!e2.dirs.save_dir.join("big.json").exists());
}

// ---------------------------------------------------------------- API surface

#[test]
fn every_spec_function_exists_in_lua() {
    let e = env();
    let perms = bm_script::manifest::PERMISSIONS.iter().map(|p| format!("\"{p}\"")).collect::<Vec<_>>().join(",");
    let mut lua = String::from("local missing = {}\n");
    for f in bm_script::spec::API_FUNCTIONS {
        let target = if f.module == "mod" { "mod".to_string() } else { f.module.to_string() };
        lua.push_str(&format!(
            "if type({target}) ~= 'table' or type({target}.{n}) ~= 'function' then missing[#missing+1] = '{target}.{n}' end\n",
            n = f.name
        ));
    }
    lua.push_str("log.info('MISSING:' .. table.concat(missing, ','))\nif type(print) ~= 'function' or type(require) ~= 'function' then log.error('no print/require') end");
    script_mod(&e.mods_root, "all", &perms, &lua);
    let (_rt, host, reports) = boot(&e, Default::default());
    assert!(reports[0].ok, "{reports:?}");
    assert!(host.log_text().contains("MISSING:\n") || host.log_text().ends_with("MISSING:"), "{}", host.log_text());
}

#[test]
fn stubs_cover_the_api() {
    let s = bm_script::spec::lua_stubs();
    assert!(s.starts_with("---@meta"));
    for f in bm_script::spec::API_FUNCTIONS.iter().filter(|f| f.module != "mod") {
        assert!(s.contains(&format!("function {}.{}(", f.module, f.name)), "missing {}.{}", f.module, f.name);
    }
    for ev in bm_script::events::BUILTIN_EVENTS {
        assert!(s.contains(&format!("fun(event: \"{}\"", ev.name)));
    }
    let md = bm_script::spec::api_markdown();
    assert!(md.contains("entity.spawn") && md.contains("on_damage"));
}
