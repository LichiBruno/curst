//! the keymap preset and the theme: changed from the settings, or chosen
//! in the welcome on the first run, and saved to `config.toml`

use super::{Action, Command, Keymap, Popup, State};
use crate::theme::Theme;

/// a step of the welcome shown on the first run
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    Keys,
    Theme,
}

/// the presets, with what each is like
pub(crate) const PRESETS: [(&str, &str); 2] = [
    ("standard", "arrows, Enter and a letter for each action"),
    ("vim", "standard, plus h j k l, g g, G, d d, : and more"),
];

/// the keymap for `preset` with the user's own bindings on top
fn use_preset(state: &mut State, preset: &str) {
    let (keymap, warnings) = Keymap::from_settings(Some(preset), &state.key_bindings);

    state.keymap = keymap;
    preset.clone_into(&mut state.keys_preset);

    if let Some(first) = warnings.first() {
        state.notice = Some(format!("config.toml: {first}"));
    }
}

/// makes the theme at `index` of the known ones the current one
fn use_theme(state: &mut State, index: usize) {
    if let Some(theme) = state.themes.get(index) {
        state.theme = theme.clone();
    }
}

fn preset_index(state: &State) -> usize {
    PRESETS
        .iter()
        .position(|(name, _)| *name == state.keys_preset)
        .unwrap_or(0)
}

pub(crate) fn theme_index(state: &State) -> usize {
    state
        .themes
        .iter()
        .position(|t| t.name == state.theme.name)
        .unwrap_or(0)
}

/// the next preset, or theme, after the current one, wrapping around
fn cycle(current: usize, count: usize, forward: bool) -> usize {
    let count = count.max(1);

    if forward {
        (current + 1) % count
    } else {
        (current + count - 1) % count
    }
}

/// saves the preset when `keys`, and the theme when `theme`
fn save(state: &State, keys: bool, theme: bool) -> Command {
    Command::SavePreferences {
        keys: keys.then(|| state.keys_preset.clone()),
        theme: theme.then(|| state.theme.name.clone()),
    }
}

/// the settings keys that change the keymap or the theme, saved at once
pub(crate) fn settings(state: &mut State, action: &Action) -> Option<Vec<Command>> {
    match action {
        Action::NextKeys => {
            let next = cycle(preset_index(state), PRESETS.len(), true);

            use_preset(state, PRESETS[next].0);

            Some(vec![save(state, true, false)])
        }
        Action::NextTheme => {
            let next = cycle(theme_index(state), state.themes.len(), true);

            use_theme(state, next);

            Some(vec![save(state, false, true)])
        }
        _ => None,
    }
}

/// opens the welcome over whatever pop-up was waiting, which comes back
/// once the welcome is done
pub(crate) fn welcome(state: &mut State) {
    let then = state.popup.take().map(Box::new);

    state.popup = Some(Popup::Welcome {
        step: Step::Keys,
        then,
    });
}

/// a key in the welcome: up and down try a choice at once, Enter goes on,
/// Esc keeps what is chosen; either way the choices are saved, so the
/// welcome is not shown again
pub(crate) fn welcome_key(
    state: &mut State,
    step: Step,
    then: Option<Box<Popup>>,
    action: Action,
) -> Result<Vec<Command>, Action> {
    let forward = matches!(action, Action::Down);

    match (step, action) {
        (Step::Keys, Action::Up | Action::Down) => {
            let next = cycle(preset_index(state), PRESETS.len(), forward);

            use_preset(state, PRESETS[next].0);
        }
        (Step::Theme, Action::Up | Action::Down) => {
            let next = cycle(theme_index(state), state.themes.len(), forward);

            use_theme(state, next);
        }
        (Step::Keys, Action::ConfirmPopup) => {
            state.popup = Some(Popup::Welcome {
                step: Step::Theme,
                then,
            });

            return Ok(Vec::new());
        }
        (_, Action::ConfirmPopup | Action::ClosePopup) => {
            state.popup = then.map(|p| *p);
            state.notice = Some(format!(
                "saved to config.toml, {} changes it; {} lists every key",
                state.key(super::Layer::Global, "settings"),
                state.key(super::Layer::Global, "help")
            ));

            return Ok(vec![save(state, true, true)]);
        }
        (_, other) => {
            state.popup = Some(Popup::Welcome { step, then });

            return Err(other);
        }
    }

    state.popup = Some(Popup::Welcome { step, then });

    Ok(Vec::new())
}

/// every theme that can be chosen: the built-in ones, then the user's
pub(crate) fn known_themes(user: Vec<Theme>) -> Vec<Theme> {
    let mut themes: Vec<Theme> = crate::theme::BUILT_IN
        .iter()
        .filter_map(|name| Theme::built_in(name))
        .collect();

    themes.extend(user);

    themes
}
