//! Scenario 3: runtime config reload - start with the default theme,
//! modify config.toml externally, reload, verify the theme changed.

use crate::common::env_lock;
use mujo::config::Config;
use tempfile::TempDir;

#[test]
fn test_config_reload() {
    let _lock = env_lock();
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("config.toml");

    // Redirect Config::save()'s default location ($HOME/.config/mujo)
    // into the temp dir so the test never touches the real user config.
    std::env::set_var("HOME", dir.path());

    // "Start mujo": the first load creates a default config file.
    let config = Config::load_or_create(&path).expect("default config should load");
    assert!(path.exists(), "a default config.toml should be created");
    let theme = config.effective_theme();
    assert_eq!(theme.accent, "#C7F542", "default theme should be lime");
    // transparent defaults to false, so the preset hex background is kept.
    assert_eq!(theme.background, "#2A2A2A");

    // Modify config.toml externally: switch the theme to dracula.
    std::fs::write(&path, "[colors]\ntheme = \"dracula\"\n").expect("write dracula config");

    // Trigger the reload (the same call App::reload_config makes) and
    // verify the resolved theme changed.
    let reloaded = Config::load_or_create(&path).expect("reloaded config should parse");
    let theme = reloaded.effective_theme();
    assert_eq!(theme.foreground, "#F8F8F2");
    assert_eq!(theme.secondary, "#6272A4");
    assert_eq!(theme.accent, "#BD93F9");
    assert_eq!(theme.background, "#282A36");

    // The reload path also persists TUI-side changes (e.g. the git remote
    // URL) back to config.toml. save() writes to Config::config_path()
    // ($HOME/.config/mujo/config.toml on all platforms), which the HOME
    // env var above redirects into the temp dir - NOT the explicit `path`
    // used by load_or_create. Verify the round trip at the real location.
    let mut config = reloaded;
    config.git.remote_url = "git@github.com:user/repo.git".to_string();
    config.save().expect("save should persist the config");

    let saved_path = dir.path().join(".config").join("mujo").join("config.toml");
    assert!(
        saved_path.exists(),
        "save() should write to $HOME/.config/mujo/config.toml"
    );
    let persisted = Config::load_or_create(&saved_path).expect("reload after save");
    assert_eq!(persisted.git.remote_url, "git@github.com:user/repo.git");
    assert_eq!(persisted.effective_theme().accent, "#BD93F9");
}
