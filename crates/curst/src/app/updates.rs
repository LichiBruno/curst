//! looking for new versions: off until turned on in Settings, and kept in
//! `config.toml`

use super::{Command, State};

/// turns looking for new versions on or off; on, it looks at once
pub(crate) fn toggle(state: &mut State) -> Vec<Command> {
    let on = state.check_updates != Some(true);

    state.check_updates = Some(on);

    let mut commands = vec![Command::SaveCheckUpdates(on)];

    if on {
        commands.push(Command::CheckUpdates);
    }

    commands
}

/// a newer version is out: said now, and kept for Settings
pub(crate) fn found(state: &mut State, news: String) {
    state.notice = Some(news.clone());
    state.newer = Some(news);
}
