//! Loose-file override tests that need no game files.
use bm_sound::pcm::wav_i16;
use bm_sound::SoundLibrary;
use std::sync::Arc;

fn tmpdir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("bm_sound_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn mod_only_sounds_are_found_by_name_and_decoded() {
    let game = tmpdir("game");
    let modd = tmpdir("mod");
    std::fs::create_dir_all(game.join("Scenes")).unwrap();
    let wav = wav_i16(&(0..4410).map(|i| ((i as f32 * 0.2).sin() * 10000.0) as i16).collect::<Vec<_>>(), 1, 44100);
    std::fs::create_dir_all(modd.join("Sounds/_global/Custom/Cats")).unwrap();
    std::fs::write(modd.join("Sounds/_global/Custom/Cats/Meow.wav"), &wav).unwrap();
    let mut vfs = bm_vfs::Vfs::new(&game).unwrap();
    vfs.add_mod_dir(&modd).unwrap();
    let lib = SoundLibrary::new(Arc::new(vfs));
    let p = lib.pcm_by_name("Custom/Cats/Meow.wav").unwrap();
    assert_eq!(p.frames(), 4410);
    assert_eq!(p.sample_rate, 44100);
    assert!(p.peak() > 0.25);
    // second call hits the cache
    let _ = lib.pcm_by_name(r"custom\cats\meow").unwrap();
    assert_eq!(lib.cache_stats().hits, 1);
    assert_eq!(lib.list_mod_sounds(), vec!["Sounds/_global/Custom/Cats/Meow.wav".to_string()]);
    assert!(lib.pcm_by_name("Custom/Cats/Nothing").is_err());
    let _ = std::fs::remove_dir_all(game);
    let _ = std::fs::remove_dir_all(modd);
}
