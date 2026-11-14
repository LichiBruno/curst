use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Margin, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};

use super::request::table;
use super::{field, pane, tab_regions, tab_titles};
use curst_core::model::{Auth, LocalId};
use curst_core::secrets::{Approval, SecretKey};
use curst_core::store::Backend;

use ratatui_textarea::TextArea;

use unicode_width::UnicodeWidthStr;

use crate::app::{
    AuthDialog, Entry, Field, Fill, Hint, Hints, Layer, NodeKey, PRESETS, Palette, Popup, Review,
    ReviewKind, SaveDialog, SpecPreview, State, Step, Target, VarTab, VariablesEditor,
    WorkflowEditor, WorkflowRow, display,
};

/// a one-line prompt titled `title`
fn prompt(state: &State, title: &str, input: &TextArea<'static>, frame: &mut Frame) {
    let area = centered(frame, 60, 3);
    let block = pane(&state.theme, Line::from(format!(" {title} ")), true);
    let inner = block.inner(area);

    frame.render_widget(Clear, area);
    state.region(area, Target::Popup);
    frame.render_widget(block, area);
    state.region(inner, Target::Text(Field::Popup));
    frame.render_widget(&*field(&state.theme, input, None), inner);
}

/// a box of at most `width` by `height` in the middle of the screen
fn centered(frame: &Frame, width: u16, height: u16) -> Rect {
    let [area] = Layout::vertical([Constraint::Max(height)])
        .flex(Flex::Center)
        .areas(frame.area());
    let [area] = Layout::horizontal([Constraint::Max(width)])
        .flex(Flex::Center)
        .areas(area);

    area
}

pub(super) fn draw(state: &State, popup: &Popup, frame: &mut Frame) {
    let theme = &state.theme;

    match popup {
        Popup::Prompt { purpose, input } => prompt(state, purpose.title(), input, frame),
        Popup::Confirm { message, .. } => {
            let area = centered(frame, 60, 5);
            let block = pane(theme, Line::from(" Are you sure? "), true);
            let text = vec![
                Line::from(display(message)),
                Line::from(""),
                Line::from(vec![
                    key_span(state, Layer::Confirm, "confirm"),
                    Span::raw("yes   "),
                    key_span(state, Layer::Confirm, "close"),
                    Span::raw("no"),
                ]),
            ];

            frame.render_widget(Clear, area);
            state.region(area, Target::Popup);
            frame.render_widget(
                Paragraph::new(text).wrap(Wrap { trim: true }).block(block),
                area,
            );
        }
        Popup::Environments {
            collection,
            selected,
        } => {
            let Some(workspace) = state.workspace(collection) else {
                return;
            };

            let environments = &workspace.collection.environments;
            let height = u16::try_from(environments.len() + 3).unwrap_or(u16::MAX);
            let area = centered(frame, 44, height);
            let title = format!(" Environments · {} ", display(&workspace.collection.name));
            let block = pane(theme, Line::from(title), true);
            let names = std::iter::once((None, "No environment".to_owned()))
                .chain(environments.iter().map(|e| (Some(&e.id), display(&e.name))));
            let mut lines: Vec<Line> = names
                .enumerate()
                .map(|(index, (id, name))| {
                    let active = id == workspace.active_environment.as_ref();
                    let mark = if active { "✓ " } else { "  " };
                    let style = if index == *selected {
                        theme.selected
                    } else {
                        Style::default()
                    };

                    Line::from(Span::styled(format!("{mark}{name}"), style))
                })
                .collect();

            if environments.is_empty() {
                lines.push(Line::from(Span::styled(
                    format!(
                        "  press {} to add one",
                        state.key(Layer::Environments, "env.new")
                    ),
                    theme.muted,
                )));
            }

            frame.render_widget(Clear, area);
            state.region(area, Target::Popup);

            for index in 0..=environments.len() {
                line_region(state, block.inner(area), index, Target::Item(index));
            }

            frame.render_widget(Paragraph::new(lines).block(block), area);
        }
        Popup::Trust { collection } => trust(state, collection, frame),
        Popup::ChooseBackend => choose_backend(state, frame),
        Popup::Passphrase {
            new,
            first,
            input,
            error,
            ..
        } => passphrase(state, *new, first.is_some(), input, error.as_deref(), frame),
        Popup::Settings => settings(state, frame),
        Popup::Welcome { step, .. } => welcome(state, *step, frame),
        Popup::Tabs { selected } => tab_list(state, *selected, frame),
        Popup::Help { scroll, here } => help(state, *scroll, here, frame),
        Popup::Palette(palette) => palette_list(state, palette, frame),
        Popup::Fill(_)
        | Popup::SaveVariables { .. }
        | Popup::SpecPreview(_)
        | Popup::Review(_)
        | Popup::SaveRequest(_)
        | Popup::CollectionSettings { .. }
        | Popup::Auth(_)
        | Popup::Run
        | Popup::Workflows { .. }
        | Popup::WorkflowEditor(_)
        | Popup::SpecChoice { .. } => draw_import(state, popup, frame),
        Popup::Approve(approval) => approve(state, approval, frame),
        Popup::Variables(editor) => variables(state, editor, frame),
    }
}

fn variables(state: &State, editor: &VariablesEditor, frame: &mut Frame) {
    let theme = &state.theme;
    let area = centered(frame, 100, frame.area().height.saturating_sub(4).max(8));
    let label = |tab: VarTab| match (tab, &editor.environment) {
        (VarTab::Environment, Some(_)) => "Environment",
        (tab, _) => tab.label(),
    };
    let mut title = vec![Span::raw(" Variables")];

    title.extend(tab_titles(theme, &editor.tabs(), editor.tab, label));

    let tabs = title.clone();

    if let (VarTab::Environment, Some(environment)) = (editor.tab, &editor.environment) {
        title.push(Span::raw(format!("· {} ", display(&environment.name))));
    }

    let block = pane(theme, Line::from(title), true);
    let inner = block.inner(area);
    let add = state.key(Layer::Variables, "row.add");
    let empty = match editor.tab {
        VarTab::Environment => {
            format!("no variables in this environment, press {add} to add one")
        }
        VarTab::Collection => format!("no collection variables, press {add} to add one"),
        VarTab::Globals => format!("no global variables, press {add} to add one"),
    };

    frame.render_widget(Clear, area);
    state.region(area, Target::Popup);
    tab_regions(state, area, &tabs, 2, Target::PopupTab);
    frame.render_widget(block, area);
    table(
        state,
        editor.table(),
        ["Variable", "Value"],
        true,
        &empty,
        frame,
        inner,
    );
}

/// the open tabs, numbered as the keys that reach them: 1 to 8 for the
/// first eight, and 9 for the last
fn tab_list(state: &State, selected: usize, frame: &mut Frame) {
    let theme = &state.theme;
    let labels = state.tab_labels();
    let last = labels.len() - 1;
    let dim = theme.muted;
    let key = |name: &str| state.keymap.key_for(Layer::Tabs, name);
    let numbers: Vec<String> = (0..labels.len())
        .map(|index| {
            let name = match index {
                _ if index == last && index >= 8 => "request.go-last".to_owned(),
                0..8 => format!("request.go-{}", index + 1),
                _ => return String::new(),
            };

            key(&name).unwrap_or_default()
        })
        .collect();
    let width = numbers.iter().map(|n| n.width()).max().unwrap_or(0);
    let lines: Vec<Line> = labels
        .iter()
        .enumerate()
        .map(|(index, label)| {
            let number = &numbers[index];
            let number = format!("{number}{}", " ".repeat(width - number.width()));
            let current = if label.active { "•" } else { " " };
            let dirty = if label.dirty { " ●" } else { "" };
            let text = format!(
                " {number} {current} {:<7} {}{dirty} ",
                label.method.as_str(),
                display(&label.name)
            );
            let style = if index == selected {
                theme.selected
            } else {
                Style::default()
            };

            Line::from(vec![Span::styled(text, style)])
        })
        .chain(std::iter::once(Line::from(Span::styled(
            " • current tab",
            dim,
        ))))
        .collect();
    let height = u16::try_from(lines.len() + 2).unwrap_or(u16::MAX);
    let area = centered(frame, 60, height);

    frame.render_widget(Clear, area);
    state.region(area, Target::Popup);

    for index in 0..labels.len() {
        line_region(
            state,
            area.inner(Margin::new(1, 1)),
            index,
            Target::Item(index),
        );
    }

    frame.render_widget(
        Paragraph::new(lines).block(pane(theme, Line::from(" Tabs "), true)),
        area,
    );
}

/// the pop-ups that import, refresh, fill in and save
fn draw_import(state: &State, popup: &Popup, frame: &mut Frame) {
    match popup {
        Popup::Fill(fill) => fill_list(state, fill, frame),
        Popup::SaveVariables { quit, .. } => save_question(state, *quit, frame),
        Popup::SpecPreview(preview) => spec_preview(state, preview, frame),
        Popup::Review(review) => refresh_review(state, review, frame),
        Popup::SaveRequest(dialog) => save_dialog(state, dialog, frame),
        Popup::CollectionSettings { target, selected } => {
            collection_settings(state, target, *selected, frame);
        }
        Popup::Auth(dialog) => auth_dialog(state, dialog, frame),
        Popup::Run => run(state, frame),
        Popup::Workflows {
            collection,
            selected,
        } => workflows(state, collection, *selected, frame),
        Popup::WorkflowEditor(editor) => workflow_editor(state, editor, frame),
        Popup::SpecChoice {
            folder,
            specs,
            selected,
        } => spec_choice(state, folder, specs, *selected, frame),
        _ => {}
    }
}

/// the settings of one collection, each row acted on with Enter
fn collection_settings(state: &State, target: &NodeKey, selected: usize, frame: &mut Frame) {
    use crate::app::CollectionRow as Row;

    let Some(workspace) = state.workspace(&target.collection) else {
        return;
    };

    let rows = Row::of(target);
    let name = if target.path.is_empty() {
        workspace.collection.name.as_str()
    } else {
        workspace
            .collection
            .item(&target.path)
            .map_or("", |item| item.name())
    };
    let theme = &state.theme;
    let area = centered(frame, 72, u16::try_from(rows.len() + 2).unwrap_or(7));
    let inner = area.inner(Margin::new(1, 1));
    let width = usize::from(inner.width);
    let source = workspace.collection.source.as_ref();
    let row = |index: usize, text: &str, note: Option<(String, Style)>| {
        let style = if index == selected {
            theme.selected
        } else {
            Style::default()
        };
        let text = format!(" {text} ");
        let mut spans = vec![Span::styled(text.clone(), style)];

        // a long spec location is cut, so each row stays one line
        if let Some((note, note_style)) = note {
            let room = width.saturating_sub(text.width() + 1);
            let note = if note.width() > room {
                let kept: String = note.chars().take(room.saturating_sub(1)).collect();

                format!("{kept}…")
            } else {
                note
            };

            spans.push(Span::styled(format!(" {note}"), note_style));
        }

        Line::from(spans)
    };
    let lines: Vec<Line> = rows
        .iter()
        .enumerate()
        .map(|(index, kind)| match kind {
            Row::Auth => row(
                index,
                "Auth",
                Some((auth_summary(state, target), theme.muted)),
            ),
            Row::InheritAll => {
                let own = workspace.collection.own_auth().len();

                row(
                    index,
                    "Make every request inherit",
                    Some((format!("{own} set their own"), theme.muted)),
                )
            }
            Row::Workflows => {
                let count = workspace.collection.workflows.len();

                row(index, "Workflows", Some((count.to_string(), theme.muted)))
            }
            Row::AutoSave => {
                let (on, style) = if workspace.auto_save {
                    ("on", theme.success)
                } else {
                    ("off", theme.muted)
                };

                row(index, "Save edits without asking", Some((on.into(), style)))
            }
            Row::Refresh => row(
                index,
                "Refresh from the spec",
                source
                    .is_none()
                    .then(|| ("not imported from a spec".into(), theme.muted)),
            ),
            Row::Source => row(
                index,
                "Change where the spec is",
                source.map(|s| (display(&s.location), theme.muted)),
            ),
            Row::Export => row(index, "Export as a bundle file", None),
        })
        .collect();
    let block = pane(theme, Line::from(format!(" {} ", display(name))), true);

    frame.render_widget(Clear, area);
    state.region(area, Target::Popup);
    frame.render_widget(block, area);

    for index in 0..rows.len() {
        line_region(state, inner, index, Target::Item(index));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

/// a list of lines in a box, `selected` highlighted, each a click target;
/// the list scrolls to keep `selected` in sight
fn list_box(
    state: &State,
    frame: &mut Frame,
    title: String,
    lines: Vec<Line<'static>>,
    selected: Option<usize>,
) {
    let theme = &state.theme;
    let height = frame.area().height.saturating_sub(4).max(5);
    let shown = usize::from(height.saturating_sub(2)).max(1);
    let first = selected.unwrap_or(0).saturating_sub(shown - 1);
    let rows = u16::try_from(lines.len().min(shown) + 2).unwrap_or(height);
    let area = centered(frame, 80, rows);
    let inner = area.inner(Margin::new(1, 1));
    let count = lines.len();
    let lines: Vec<Line> = lines
        .into_iter()
        .enumerate()
        .skip(first)
        .take(shown)
        .map(|(index, line)| {
            if Some(index) == selected {
                line.style(theme.selected)
            } else {
                line
            }
        })
        .collect();

    frame.render_widget(Clear, area);
    state.region(area, Target::Popup);

    for index in first..count.min(first + shown) {
        line_region(state, inner, index - first, Target::Item(index));
    }

    frame.render_widget(
        Paragraph::new(lines).block(pane(theme, Line::from(title), true)),
        area,
    );
}

/// the workflows of a collection, and a row to make a new one
fn workflows(state: &State, collection: &LocalId, selected: usize, frame: &mut Frame) {
    let Some(workspace) = state.workspace(collection) else {
        return;
    };

    let theme = &state.theme;
    let mut lines: Vec<Line> = workspace
        .collection
        .workflows
        .iter()
        .map(|w| {
            let steps = match w.steps.len() {
                1 => "1 step".to_owned(),
                n => format!("{n} steps"),
            };

            Line::from(vec![
                Span::raw(format!(" {} ", display(&w.name))),
                Span::styled(steps, theme.muted),
            ])
        })
        .collect();

    lines.push(Line::from(Span::styled(" + new workflow", theme.muted)));

    let title = format!(" Workflows of {} ", display(&workspace.collection.name));

    list_box(state, frame, title, lines, Some(selected));
}

/// the workflow running: each step and how far it got, and why it stopped
fn run(state: &State, frame: &mut Frame) {
    use crate::app::StepState;

    let Some(run) = &state.run else {
        return;
    };

    let theme = &state.theme;
    let workspace = state.workspace(&run.collection);
    let mut lines: Vec<Line> = run
        .workflow
        .steps
        .iter()
        .zip(&run.states)
        .map(|(step, done)| {
            let name = workspace
                .and_then(|w| w.collection.item(&step.request))
                .map_or_else(|| step.request.join("/"), |item| item.name().to_owned());
            let (mark, style) = match done {
                StepState::Waiting => ("·".to_owned(), theme.muted),
                StepState::Running => ("…".to_owned(), theme.warning),
                StepState::Done(status) => (status.to_string(), theme.success),
                StepState::Failed => ("✗".to_owned(), theme.error),
            };
            let kept: Vec<String> = step
                .keep
                .iter()
                .map(|c| format!("{{{{{}}}}}", c.variable))
                .collect();
            let mut spans = vec![
                Span::styled(format!(" {mark:>3} "), style),
                Span::raw(display(&name)),
            ];

            if !kept.is_empty() {
                spans.push(Span::styled(format!("  → {}", kept.join(" ")), theme.muted));
            }

            Line::from(spans)
        })
        .collect();

    if let Some(error) = &run.error {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!(" {}", display(error)),
            theme.error,
        )));
    } else if run.finished() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(" done", theme.success)));
    }

    // a long error wraps, and the box grows to show all of it
    let width = usize::from(frame.area().width.min(80).saturating_sub(2)).max(1);
    let rows: usize = lines.iter().map(|l| l.width().div_ceil(width).max(1)).sum();
    let area = centered(frame, 80, u16::try_from(rows + 2).unwrap_or(u16::MAX));

    frame.render_widget(Clear, area);
    state.region(area, Target::Popup);
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(pane(
            theme,
            Line::from(format!(" {} ", display(&run.workflow.name))),
            true,
        )),
        area,
    );
}

/// a workflow being edited: its name, each step's request and the values
/// it keeps; or the requests to choose a step's from
fn workflow_editor(state: &State, editor: &WorkflowEditor, frame: &mut Frame) {
    let theme = &state.theme;

    if let Some((step, selected)) = editor.picking {
        let lines = editor
            .requests
            .iter()
            .map(|(_, label)| Line::from(format!(" {}", display(label))))
            .collect();

        list_box(
            state,
            frame,
            format!(" Request of step {} ", step + 1),
            lines,
            Some(selected),
        );

        return;
    }

    let rows = editor.rows();
    let height = frame.area().height.saturating_sub(4).max(6);
    let shown = usize::from(height.saturating_sub(2)).max(1);
    let first = editor.row.saturating_sub(shown - 1);
    let area = centered(
        frame,
        90,
        u16::try_from(rows.len().min(shown) + 2).unwrap_or(height),
    );
    let inner = area.inner(Margin::new(1, 1));

    frame.render_widget(Clear, area);
    state.region(area, Target::Popup);
    frame.render_widget(pane(theme, Line::from(" Workflow "), true), area);

    for (index, row) in rows.iter().enumerate().skip(first).take(shown) {
        let Ok(offset) = u16::try_from(index - first) else {
            break;
        };
        let line = Rect {
            y: inner.y + offset,
            height: 1,
            ..inner
        };
        let selected = index == editor.row;

        line_region(state, inner, index - first, Target::Item(index));
        editor_row(state, editor, *row, line, selected, frame);
    }
}

/// one row of the workflow editor, drawn on `line`
fn editor_row(
    state: &State,
    editor: &WorkflowEditor,
    row: WorkflowRow,
    line: Rect,
    selected: bool,
    frame: &mut Frame,
) {
    use crate::app::WorkflowRow as Row;

    let theme = &state.theme;
    let style = if selected {
        theme.selected
    } else {
        Style::default()
    };

    match row {
        Row::Name => {
            let [label, field_area] =
                Layout::horizontal([Constraint::Length(10), Constraint::Fill(1)]).areas(line);

            frame.render_widget(Paragraph::new(Span::styled(" Name", style)), label);

            if selected {
                state.region(field_area, Target::Text(Field::Popup));
            }

            frame.render_widget(&*field(theme, &editor.name, None), field_area);
        }
        Row::Request(step) => {
            let request = &editor.steps[step].request;
            let label = editor
                .label(request)
                .map_or_else(|| "choose a request".to_owned(), display);
            let label_style = if request.is_empty() {
                theme.muted
            } else {
                Style::default()
            };

            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(format!(" {:<3}", step + 1), style),
                    Span::styled(label, label_style),
                ])),
                line,
            );
        }
        Row::Capture(step, i) => {
            let capture = &editor.steps[step].keep[i];
            let [gap, variable, arrow, from] = Layout::horizontal([
                Constraint::Length(6),
                Constraint::Length(20),
                Constraint::Length(3),
                Constraint::Fill(1),
            ])
            .areas(line);

            frame.render_widget(Paragraph::new(Span::styled(" ", style)), gap);
            frame.render_widget(&*field(theme, &capture.variable, None), variable);
            frame.render_widget(Paragraph::new(Span::styled(" ← ", theme.muted)), arrow);
            frame.render_widget(&*field(theme, &capture.from, None), from);

            if selected {
                let at = if editor.column == 0 { variable } else { from };

                state.region(at, Target::Text(Field::Popup));
            }
        }
        Row::AddCapture(_) => frame.render_widget(
            Paragraph::new(Span::styled(
                "      + keep a value",
                if selected { style } else { theme.muted },
            )),
            line,
        ),
        Row::AddStep => frame.render_widget(
            Paragraph::new(Span::styled(
                " + add a step",
                if selected { style } else { theme.muted },
            )),
            line,
        ),
    }
}

/// the auth a collection or folder sets, and for one that inherits, what
fn auth_summary(state: &State, target: &NodeKey) -> String {
    use super::request::auth_label;

    let Some(auth) = crate::app::auth_of(state, target) else {
        return String::new();
    };

    if target.path.is_empty() || auth != Auth::Inherit {
        // a collection that inherits has nothing above it
        let auth = if auth == Auth::Inherit {
            Auth::None
        } else {
            auth
        };

        return auth_label(&auth).into();
    }

    let parent = &target.path[..target.path.len() - 1];
    let (inherited, from) = state
        .workspace(&target.collection)
        .map_or((Auth::None, String::new()), |w| {
            w.collection.inherited_auth(parent)
        });

    format!(
        "inherit: {} from {}",
        auth_label(&inherited),
        display(&from)
    )
}

/// the auth of a collection or folder: its type, then its fields
fn auth_dialog(state: &State, dialog: &AuthDialog, frame: &mut Frame) {
    use super::request::auth_label;

    let theme = &state.theme;
    let fields = dialog.editor.fields();
    let height = u16::try_from(fields.len() + 3).unwrap_or(5);
    let area = centered(frame, 72, height);
    let inner = area.inner(Margin::new(1, 1));
    let name = state
        .workspace(&dialog.target.collection)
        .and_then(|w| {
            if dialog.target.path.is_empty() {
                Some(w.collection.name.as_str())
            } else {
                w.collection
                    .item(&dialog.target.path)
                    .map(curst_core::model::Item::name)
            }
        })
        .unwrap_or("");
    let style = |row: usize| {
        if row == dialog.row {
            theme.selected
        } else {
            theme.muted
        }
    };
    let mut kind = vec![
        Span::styled(" Type        ", style(0)),
        Span::styled(
            dialog.editor.kind.label(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
    ];

    if dialog.editor.kind == crate::app::AuthKind::Inherit {
        let target = &dialog.target.path;
        let (auth, from) =
            state
                .workspace(&dialog.target.collection)
                .map_or((Auth::None, String::new()), |w| {
                    w.collection
                        .inherited_auth(&target[..target.len().saturating_sub(1)])
                });

        kind.push(Span::styled(
            format!("  {} from {}", auth_label(&auth), display(&from)),
            theme.muted,
        ));
    }

    frame.render_widget(Clear, area);
    state.region(area, Target::Popup);
    frame.render_widget(
        pane(
            theme,
            Line::from(format!(" Auth of {} ", display(name))),
            true,
        ),
        area,
    );
    line_region(state, inner, 0, Target::Item(0));
    frame.render_widget(
        Paragraph::new(Line::from(kind)),
        Rect { height: 1, ..inner },
    );

    for (index, (label, input)) in fields.into_iter().enumerate() {
        let row = index + 1;
        let Ok(offset) = u16::try_from(row) else {
            break;
        };
        let line = Rect {
            y: inner.y + offset,
            height: 1,
            ..inner
        };
        let [label_area, field_area] =
            Layout::horizontal([Constraint::Length(13), Constraint::Fill(1)]).areas(line);

        line_region(state, inner, row, Target::Item(row));

        if row == dialog.row {
            state.region(field_area, Target::Text(Field::Popup));
        }

        frame.render_widget(
            Paragraph::new(Span::styled(format!(" {label}"), style(row))),
            label_area,
        );
        frame.render_widget(&*field(theme, input, None), field_area);
    }
}

/// a new request's name, and below it where it goes; Tab opens the list
/// of every collection and folder to pick from
fn save_dialog(state: &State, dialog: &SaveDialog, frame: &mut Frame) {
    let theme = &state.theme;
    let (_, place) = &dialog.places[dialog.place];
    let height = frame.area().height.saturating_sub(4).max(6);
    let room = usize::from(height.saturating_sub(6)).max(1);
    let shown = if dialog.picking {
        dialog.places.len().min(room)
    } else {
        0
    };
    let first = dialog.selected.saturating_sub(shown.saturating_sub(1));
    let mut lines = vec![
        Line::from(""),
        Line::from(""),
        Line::from(vec![
            Span::styled("in ", theme.muted),
            Span::styled(
                display(place),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            key_span(state, Layer::SaveRequest, "tab.next"),
            Span::styled(if dialog.picking { "done" } else { "change" }, theme.muted),
        ]),
    ];

    for (index, (_, label)) in dialog.places.iter().enumerate().skip(first).take(shown) {
        let style = if index == dialog.selected {
            theme.selected
        } else {
            Style::default()
        };

        lines.push(Line::from(Span::styled(
            format!(" {}", display(label)),
            style,
        )));
    }

    let rows = u16::try_from(lines.len() + 2).unwrap_or(height).min(height);
    let area = centered(frame, 70, rows);
    let inner = area.inner(Margin::new(1, 1));
    let input = Rect { height: 1, ..inner };

    frame.render_widget(Clear, area);
    state.region(area, Target::Popup);
    line_region(state, inner, 2, Target::Run("tab.next".into()));

    for index in first..dialog.places.len().min(first + shown) {
        line_region(state, inner, 3 + index - first, Target::Item(index));
    }

    frame.render_widget(
        Paragraph::new(lines).block(pane(theme, Line::from(" Save request "), true)),
        area,
    );
    state.region(input, Target::Text(Field::Popup));
    frame.render_widget(&*field(theme, &dialog.input, None), input);
}

/// the specs a folder holds, to pick the one to import
fn spec_choice(state: &State, folder: &str, specs: &[String], selected: usize, frame: &mut Frame) {
    let theme = &state.theme;
    let height = frame.area().height.saturating_sub(4).clamp(6, 20);
    let shown = usize::from(height.saturating_sub(4));
    let first = selected.saturating_sub(shown.saturating_sub(1));
    let mut lines = vec![
        Line::from(format!("{} specs are in {}:", specs.len(), display(folder))),
        Line::from(""),
    ];

    for (index, spec) in specs.iter().enumerate().skip(first).take(shown) {
        let style = if index == selected {
            theme.selected
        } else {
            Style::default()
        };

        lines.push(Line::from(Span::styled(
            format!(" {}", display(spec)),
            style,
        )));
    }

    let rows = u16::try_from(lines.len() + 2).unwrap_or(height).min(height);
    let area = centered(frame, 80, rows);
    let inner = area.inner(Margin::new(1, 1));

    frame.render_widget(Clear, area);
    state.region(area, Target::Popup);

    for index in first..specs.len().min(first + shown) {
        line_region(state, inner, 2 + index - first, Target::Item(index));
    }

    frame.render_widget(
        Paragraph::new(lines).block(pane(theme, Line::from(" Which spec? "), true)),
        area,
    );
}

/// what a refresh found: each endpoint, ticked when it is taken, and the
/// selected one's diff, what is there now against what the spec says
fn refresh_review(state: &State, review: &Review, frame: &mut Frame) {
    use curst_core::openapi::refresh::{DiffLine, diff, request_lines};

    let theme = &state.theme;
    let height = frame.area().height.saturating_sub(4).max(10);
    let area = centered(frame, 110, height);
    let block = pane(theme, Line::from(" Refresh from the spec "), true);
    let inner = block.inner(area);
    let listed = u16::try_from(review.lines.len())
        .unwrap_or(u16::MAX)
        .min(inner.height / 2)
        .max(1);
    let [list_area, _, detail_area] = Layout::vertical([
        Constraint::Length(listed),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(inner);
    let shown = usize::from(listed);
    let first = review.selected.saturating_sub(shown.saturating_sub(1));
    let list: Vec<Line> = (first..review.lines.len().min(first + shown))
        .map(|index| {
            let mark = if review.chosen[index] { "[x]" } else { "[ ]" };
            let style = if index == review.selected {
                theme.selected
            } else if review.held[index] {
                theme.muted
            } else {
                Style::default()
            };

            Line::from(Span::styled(
                format!(" {mark} {}", display(&review.label(index))),
                style,
            ))
        })
        .collect();

    // the selected entry: its diff, what it adds, or what deleting does
    let detail: Vec<Line> = match review.lines.get(review.selected) {
        Some((ReviewKind::Changed, i)) => {
            let changed = &review.plan.changed[*i];

            diff(
                &request_lines(&changed.current),
                &request_lines(&changed.new),
            )
            .into_iter()
            .map(|line| match line {
                DiffLine::Same(text) => Line::from(format!("  {}", display(&text))),
                DiffLine::Removed(text) => {
                    Line::from(Span::styled(format!("- {}", display(&text)), theme.error))
                }
                DiffLine::Added(text) => {
                    Line::from(Span::styled(format!("+ {}", display(&text)), theme.success))
                }
            })
            .collect()
        }
        Some((ReviewKind::Added, i)) => request_lines(&review.plan.added[*i].request)
            .into_iter()
            .map(|text| Line::from(Span::styled(format!("+ {}", display(&text)), theme.success)))
            .collect(),
        Some((ReviewKind::Removed, _)) => vec![Line::from(Span::styled(
            "the spec no longer has this endpoint: taking it deletes the request",
            theme.muted,
        ))],
        None => Vec::new(),
    };
    let scroll = u16::try_from(review.scroll).unwrap_or(u16::MAX);

    frame.render_widget(Clear, area);
    state.region(area, Target::Popup);

    for index in first..review.lines.len().min(first + shown) {
        line_region(state, list_area, index - first, Target::Item(index));
    }

    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(list), list_area);
    frame.render_widget(Paragraph::new(detail).scroll((scroll, 0)), detail_area);
}

/// what a spec becomes, before anything is written
fn spec_preview(state: &State, preview: &SpecPreview, frame: &mut Frame) {
    let theme = &state.theme;
    let height = frame.area().height.saturating_sub(4).clamp(6, 24);
    let width = usize::from(frame.area().width.min(90).saturating_sub(2)).max(10);
    let shown = usize::from(height.saturating_sub(2));

    // every line wrapped to the pop-up, so a long warning is read whole
    let mut rows: Vec<Line> = Vec::new();

    for (index, text) in preview.lines().iter().enumerate() {
        let style = if index == 0 {
            Style::default().add_modifier(Modifier::BOLD)
        } else if text.starts_with("  ") {
            theme.muted
        } else {
            Style::default()
        };

        rows.extend(
            super::wrap(&display(text), width)
                .into_iter()
                .map(|row| Line::from(Span::styled(row, style))),
        );
    }

    preview.last_scroll.set(rows.len().saturating_sub(shown));

    let first = preview.scroll.min(preview.last_scroll.get());
    let lines: Vec<Line> = rows.into_iter().skip(first).take(shown).collect();
    let rows = u16::try_from(lines.len() + 2).unwrap_or(height).min(height);
    let area = centered(frame, 90, rows);
    let title = format!(" Import {} ", display(&preview.converted.bundle.name));

    frame.render_widget(Clear, area);
    state.region(area, Target::Popup);
    frame.render_widget(
        Paragraph::new(lines).block(pane(theme, Line::from(title), true)),
        area,
    );
}

/// whether to save the variables before leaving them, each answer a button
fn save_question(state: &State, quit: bool, frame: &mut Frame) {
    let theme = &state.theme;
    let question = if quit {
        "The variables have changes. Save them before quitting?"
    } else {
        "The variables have changes. Save them?"
    };
    let answers = Line::from(vec![
        key_span(state, Layer::SaveQuestion, "confirm"),
        Span::raw("save   "),
        key_span(state, Layer::SaveQuestion, "discard"),
        Span::raw("don't save   "),
        key_span(state, Layer::SaveQuestion, "close"),
        Span::raw("keep editing"),
    ]);
    let lines = vec![Line::from(question), Line::from(""), answers.clone()];
    let area = centered(frame, 64, 5);
    let inner = area.inner(Margin::new(1, 1));

    frame.render_widget(Clear, area);
    state.region(area, Target::Popup);
    button(state, inner, 2, &answers, 0, "confirm");
    button(state, inner, 2, &answers, 2, "discard");
    button(state, inner, 2, &answers, 4, "close");
    frame.render_widget(
        Paragraph::new(lines).block(pane(theme, Line::from(" Unsaved variables "), true)),
        area,
    );
}

/// the secrets to fill in, each marked once it has a value, and the field
/// for the selected one, which never shows what is typed
fn fill_list(state: &State, fill: &Fill, frame: &mut Frame) {
    let theme = &state.theme;
    let mut lines = vec![
        Line::from("These secrets have no value yet. Type each one; it goes to your"),
        Line::from("secret store, never into a file. Leave one empty to skip it."),
        Line::from(""),
    ];

    for (index, (_, label)) in fill.secrets.iter().enumerate() {
        let mark = if fill.values[index].is_some() {
            "✓"
        } else {
            " "
        };
        let style = if index == fill.selected {
            theme.selected
        } else {
            Style::default()
        };

        lines.push(Line::from(Span::styled(
            format!(" {mark} {}", display(label)),
            style,
        )));
    }

    let rows = u16::try_from(lines.len()).unwrap_or(u16::MAX);
    let area = centered(frame, 72, rows.saturating_add(4));
    let title = format!(
        " Secrets to fill in · {} of {} ",
        fill.filled(),
        fill.secrets.len()
    );
    let block = pane(theme, Line::from(title), true);
    let inner = block.inner(area);
    let [list_area, _, input_area] = Layout::vertical([
        Constraint::Length(rows),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);

    frame.render_widget(Clear, area);
    state.region(area, Target::Popup);

    for index in 0..fill.secrets.len() {
        line_region(state, list_area, 3 + index, Target::Item(index));
    }

    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(lines), list_area);
    frame.render_widget(&*field(theme, &fill.input, None), input_area);
}

/// the palette: the query, then what matches it, best first
fn palette_list(state: &State, palette: &Palette, frame: &mut Frame) {
    const SHOWN: usize = 12;

    let theme = &state.theme;
    let entries = state.palette_entries(palette);
    let dim = theme.muted;
    let rows = entries.len().clamp(1, SHOWN);
    let height = u16::try_from(rows + 3).unwrap_or(u16::MAX);
    let area = centered(frame, 72, height);
    let block = pane(theme, Line::from(" Commands and requests "), true);
    let inner = block.inner(area);
    let [input_area, list_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(inner);
    let width = usize::from(list_area.width);
    let first = palette.selected.saturating_sub(SHOWN - 1);
    let mut lines: Vec<Line> = Vec::new();

    for (index, entry) in entries.iter().enumerate().skip(first).take(SHOWN) {
        let (left, right) = match entry {
            Entry::Run(command) => (
                format!(" {}", command.what),
                format!("{} ", command.keys.as_deref().unwrap_or(&command.name)),
            ),
            Entry::Open { method, title, .. } => (
                format!(" {:<7} {}", method.as_str(), display(title)),
                "open ".to_owned(),
            ),
            Entry::Workflow { title, .. } => (
                format!(" {:<7} {}", "RUN", display(title)),
                "workflow ".to_owned(),
            ),
        };
        let gap = width.saturating_sub(left.width() + right.width()).max(1);
        let style = if index == palette.selected {
            theme.selected
        } else {
            Style::default()
        };

        lines.push(Line::from(vec![
            Span::styled(format!("{left}{}", " ".repeat(gap)), style),
            Span::styled(right, style.patch(dim)),
        ]));
    }

    if entries.is_empty() {
        lines.push(Line::from(Span::styled(" nothing matches", dim)));
    }

    frame.render_widget(Clear, area);
    state.region(area, Target::Popup);
    frame.render_widget(block, area);
    state.region(input_area, Target::Text(Field::Popup));

    for index in first..entries.len().min(first + SHOWN) {
        line_region(state, list_area, index - first, Target::Item(index));
    }

    frame.render_widget(&*field(&state.theme, &palette.input, None), input_area);
    frame.render_widget(Paragraph::new(lines), list_area);
}

fn help(state: &State, scroll: usize, here: &[Layer], frame: &mut Frame) {
    let theme = &state.theme;
    let sections = state.keymap.help(here);
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let dim = theme.muted;
    let width = sections
        .iter()
        .flat_map(|s| &s.rows)
        .map(|r| r.keys.width())
        .max()
        .unwrap_or(0)
        .min(24);
    let what = sections
        .iter()
        .flat_map(|s| &s.rows)
        .map(|r| r.what.width())
        .max()
        .unwrap_or(0)
        .min(40);
    let mut lines = Vec::new();

    for section in &sections {
        lines.push(Line::from(Span::styled(
            format!(" {}", section.title),
            bold,
        )));

        for row in &section.rows {
            let pad = width.saturating_sub(row.keys.width());

            lines.push(Line::from(vec![
                Span::styled(format!("   {}{} ", row.keys, " ".repeat(pad)), bold),
                Span::raw(format!(
                    " {}{}  ",
                    row.what,
                    " ".repeat(what.saturating_sub(row.what.width()))
                )),
                Span::styled(row.name.clone(), dim),
            ]));
        }

        lines.push(Line::default());
    }

    let height = frame.area().height.saturating_sub(4).max(8);
    let area = centered(frame, 90, height);
    let visible = usize::from(height.saturating_sub(2));
    let scroll = scroll.min(lines.len().saturating_sub(visible));
    let title = Line::from(" Keys · rebind them in config.toml, [keys.bindings] ");

    frame.render_widget(Clear, area);
    state.region(area, Target::Popup);
    frame.render_widget(
        Paragraph::new(lines)
            .scroll((u16::try_from(scroll).unwrap_or(u16::MAX), 0))
            .block(pane(theme, title, true)),
        area,
    );
}

/// records line `row` of `inner` as `target`, for clicks
fn line_region(state: &State, inner: Rect, row: usize, target: Target) {
    let Ok(row) = u16::try_from(row) else {
        return;
    };

    let line = Rect {
        y: inner.y.saturating_add(row),
        height: 1,
        ..inner
    };

    state.region(line.intersection(inner), target);
}

/// records span `index` of `line`, a key, and the span after it, what it
/// does, drawn on line `row` of `inner` without wrapping, as a button for
/// the action `name`
fn button(state: &State, inner: Rect, row: usize, line: &Line, index: usize, name: &str) {
    let x: usize = line.spans.iter().take(index).map(Span::width).sum();
    let width: usize = line.spans.iter().skip(index).take(2).map(Span::width).sum();
    let (Ok(x), Ok(row), Ok(width)) = (u16::try_from(x), u16::try_from(row), u16::try_from(width))
    else {
        return;
    };

    let span = Rect::new(
        inner.x.saturating_add(x),
        inner.y.saturating_add(row),
        width,
        1,
    );

    state.region(span.intersection(inner), Target::Run(name.to_owned()));
}

/// a key, padded, as the pop-ups list them
fn key_span(state: &State, layer: Layer, name: &str) -> Span<'static> {
    Span::styled(format!(" {} ", state.key(layer, name)), state.theme.key)
}

fn settings(state: &State, frame: &mut Frame) {
    let theme = &state.theme;
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let (now, action) = match state.backend {
        None => ("not chosen yet", "choose where to keep them"),
        Some(Backend::Keychain) => ("the system keychain", "move them to an encrypted file"),
        Some(Backend::EncryptedFile) => (
            "an encrypted file, secrets.age",
            "move them to the keychain",
        ),
    };
    let row = |label: &'static str, value: String, name: &str, what: &'static str| {
        Line::from(vec![
            Span::raw(format!("{label:<9}")),
            Span::styled(format!("{value:<16}"), bold),
            key_span(state, Layer::Settings, name),
            Span::styled(what, theme.muted),
        ])
    };
    let width = state.sidebar_width.map_or_else(
        || format!("{} columns", crate::app::AUTOMATIC_SIDEBAR),
        |w| format!("{w} columns"),
    );
    let sidebar = Line::from(vec![
        Span::raw(format!("{:<9}", "Sidebar")),
        Span::styled(format!("{width:<16}"), bold),
        key_span(state, Layer::Settings, "narrower"),
        Span::styled("narrower  ", theme.muted),
        key_span(state, Layer::Settings, "wider"),
        Span::styled("wider", theme.muted),
    ]);
    let lines = vec![
        row("Keys", state.keys_preset.clone(), "keys", "next preset"),
        row("Theme", state.theme.name.clone(), "theme", "next theme"),
        sidebar.clone(),
        Line::from(""),
        Line::from(vec![
            Span::raw(format!("{:<9}", "Secrets")),
            Span::styled(now, bold),
        ]),
        Line::from(vec![
            Span::raw(" ".repeat(9)),
            key_span(state, Layer::Settings, "migrate"),
            Span::raw(action),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Changes are saved to config.toml at once.",
            theme.muted,
        )),
    ];
    let area = centered(frame, 80, 10);
    let inner = area.inner(Margin::new(1, 1));

    frame.render_widget(Clear, area);
    state.region(area, Target::Popup);

    // each setting's line changes it; the sidebar's keys are buttons
    for (row, name) in [(0, "keys"), (1, "theme"), (5, "migrate")] {
        line_region(state, inner, row, Target::Run(name.to_owned()));
    }

    button(state, inner, 2, &sidebar, 2, "narrower");
    button(state, inner, 2, &sidebar, 4, "wider");

    frame.render_widget(
        Paragraph::new(lines).block(pane(theme, Line::from(" Settings "), true)),
        area,
    );
}

/// the first run: how keys work, then how it looks, each tried at once
fn welcome(state: &State, step: Step, frame: &mut Frame) {
    let theme = &state.theme;
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let choice = |chosen: bool, name: &str, what: &str| {
        let mark = if chosen { "›" } else { " " };
        let style = if chosen {
            theme.selected
        } else {
            Style::default()
        };

        Line::from(vec![
            Span::raw(format!("  {mark} ")),
            Span::styled(format!(" {name:<10}"), style),
            Span::styled(format!("  {what}"), theme.muted),
        ])
    };
    let mut lines = vec![
        Line::from(Span::styled("Welcome to curst", bold)),
        Line::from(""),
    ];

    match step {
        Step::Keys => {
            lines.push(Line::from("How do you like to move around?"));
            lines.push(Line::from(""));

            for (name, what) in PRESETS {
                lines.push(choice(state.keys_preset == name, name, what));
            }
        }
        Step::Theme => {
            lines.push(Line::from(
                "Which colors suit your terminal? It changes as you choose.",
            ));
            lines.push(Line::from(""));

            for option in &state.themes {
                let what = match option.name.as_str() {
                    "dark" => "for a dark background",
                    "light" => "for a light background",
                    "mono" => "no color at all",
                    _ => "yours, from the themes folder",
                };

                lines.push(choice(state.theme.name == option.name, &option.name, what));
            }
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Both can be changed later in the settings, and every key in config.toml.",
        theme.muted,
    )));

    let height = u16::try_from(lines.len() + 2).unwrap_or(u16::MAX);
    let area = centered(frame, 78, height);
    let title = match step {
        Step::Keys => " Keys · 1 of 2 ",
        Step::Theme => " Theme · 2 of 2 ",
    };
    let choices = match step {
        Step::Keys => PRESETS.len(),
        Step::Theme => state.themes.len(),
    };

    frame.render_widget(Clear, area);
    state.region(area, Target::Popup);

    // the choices come after the greeting and the question
    for index in 0..choices {
        line_region(
            state,
            area.inner(Margin::new(1, 1)),
            4 + index,
            Target::Item(index),
        );
    }

    frame.render_widget(
        Paragraph::new(lines).block(pane(theme, Line::from(title), true)),
        area,
    );
}

fn choose_backend(state: &State, frame: &mut Frame) {
    let theme = &state.theme;
    let lines = vec![
        Line::from("This request uses a secret. Where should curst keep secrets?"),
        Line::from(""),
        Line::from(vec![
            key_span(state, Layer::Backend, "backend.keychain"),
            Span::raw("the system keychain: nothing to type, but it stays on"),
        ]),
        Line::from("    this machine, and some Linux setups have none"),
        Line::from(vec![
            key_span(state, Layer::Backend, "backend.file"),
            Span::raw("an encrypted file: works everywhere, asks for a"),
        ]),
        Line::from("    passphrase once per session"),
        Line::from(""),
        Line::from(Span::styled(
            "You can switch later in Settings.",
            theme.muted,
        )),
    ];
    let area = centered(frame, 68, 10);
    let inner = area.inner(Margin::new(1, 1));

    frame.render_widget(Clear, area);
    state.region(area, Target::Popup);

    // each choice takes two lines
    for (row, name) in [(2, "backend.keychain"), (4, "backend.file")] {
        line_region(state, inner, row, Target::Run(name.to_owned()));
        line_region(state, inner, row + 1, Target::Run(name.to_owned()));
    }

    frame.render_widget(
        Paragraph::new(lines).block(pane(theme, Line::from(" Keep secrets where? "), true)),
        area,
    );
}

fn passphrase(
    state: &State,
    new: bool,
    confirming: bool,
    input: &TextArea<'static>,
    error: Option<&str>,
    frame: &mut Frame,
) {
    let title = match (new, confirming) {
        (true, false) => " Choose a passphrase for secrets.age ",
        (true, true) => " Type the passphrase again ",
        (false, _) => " Passphrase for secrets.age ",
    };
    let theme = &state.theme;
    let area = centered(frame, 60, 4);
    let block = pane(theme, Line::from(title), true);
    let inner = block.inner(area);
    let [field, message] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(inner);

    frame.render_widget(Clear, area);
    state.region(area, Target::Popup);
    frame.render_widget(block, area);
    frame.render_widget(&*super::field(theme, input, None), field);

    if let Some(error) = error {
        frame.render_widget(
            Paragraph::new(Span::styled(error.to_owned(), theme.warning)),
            message,
        );
    }
}

fn approve(state: &State, approval: &Approval, frame: &mut Frame) {
    let theme = &state.theme;

    let bold = Style::default().add_modifier(Modifier::BOLD);
    let names: Vec<String> = approval
        .keys
        .iter()
        .map(|key| {
            let scope = match key {
                SecretKey::Environment { .. } => "environment",
                SecretKey::Collection { .. } => "collection",
                SecretKey::Global { .. } | SecretKey::Probe => "global",
            };

            format!(
                "{} ({scope})",
                key.name().map_or_else(String::new, ToString::to_string)
            )
        })
        .collect();
    let mut lines = vec![
        Line::from("This request would send these secrets for the first time to"),
        Line::from(Span::styled(display(&approval.origin), bold)),
        Line::from(""),
        Line::from(display(&names.join(", "))),
    ];

    if approval.origin.starts_with("http://") {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "⚠ plain http: anyone on the network path can read them",
            theme.warning,
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        key_span(state, Layer::Approve, "confirm"),
        Span::raw("send them there, now and later   "),
        key_span(state, Layer::Approve, "close"),
        Span::raw("don't send"),
    ]));

    let height = u16::try_from(lines.len() + 2).unwrap_or(u16::MAX);
    let area = centered(frame, 70, height);

    frame.render_widget(Clear, area);
    state.region(area, Target::Popup);
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(pane(
            theme,
            Line::from(" Send secrets? "),
            true,
        )),
        area,
    );
}

fn trust(state: &State, collection: &LocalId, frame: &mut Frame) {
    let theme = &state.theme;
    let Some(workspace) = state.workspace(collection) else {
        return;
    };

    let bold = Style::default().add_modifier(Modifier::BOLD);
    let dim = theme.muted;
    let path = std::fs::canonicalize(&workspace.root).unwrap_or_else(|_| workspace.root.clone());
    let mut lines = vec![
        Line::from(Span::styled(display(&path.display().to_string()), bold)),
        Line::from(Span::styled(
            format!("calls itself {}", display(&workspace.collection.name)),
            dim,
        )),
        Line::from(""),
        Line::from("Requests from this folder are not sent and no secret is used"),
        Line::from("until you trust it. Trust it only if you trust where it came from."),
        Line::from(""),
        Line::from(vec![
            key_span(state, Layer::Trust, "confirm"),
            Span::raw("trust it as a new collection   "),
            key_span(state, Layer::Trust, "close"),
            Span::raw("not now, only look"),
        ]),
    ];

    if !state.orphans.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(
            "or, if it was moved from one of these folders, keep its old secrets:",
        ));

        for (index, (_, old)) in state.orphans.iter().take(9).enumerate() {
            let key = state
                .keymap
                .key_for(Layer::Trust, &format!("trust.moved-{}", index + 1))
                .unwrap_or_default();

            lines.push(Line::from(vec![
                Span::styled(format!(" {key} "), bold),
                Span::raw(display(&old.display().to_string())),
            ]));
        }
    }

    let height = u16::try_from(lines.len() + 2).unwrap_or(u16::MAX);
    let area = centered(frame, 76, height);
    let block = pane(theme, Line::from(" Trust this folder? "), true);
    let inner = block.inner(area);

    frame.render_widget(Clear, area);
    state.region(area, Target::Popup);

    // the answers, then the folders it may have been moved from
    if let Some(answers) = lines.get(6) {
        button(state, inner, 6, answers, 0, "confirm");
        button(state, inner, 6, answers, 2, "close");
    }

    for index in 0..state.orphans.len().min(9) {
        let name = format!("trust.moved-{}", index + 1);

        line_region(state, inner, 9 + index, Target::Run(name));
    }

    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// the keys of the pop-ups that import, fill in and save
fn import_hints(hints: &mut Hints, popup: &Popup) {
    match popup {
        Popup::Review(_) => {
            hints
                .pair("up", "down", "choose")
                .one("row.toggle", "take or leave")
                .pair("page-up", "page-down", "scroll the diff")
                .one("confirm", "apply")
                .one("close", "cancel");
        }
        Popup::SaveVariables { .. } => {
            hints
                .one("confirm", "save")
                .one("discard", "don't save")
                .one("close", "keep editing");
        }
        Popup::Fill(_) => {
            hints
                .one("confirm", "keep and go on")
                .pair("up", "down", "move")
                .one("close", "done");
        }
        Popup::SpecChoice { .. } => {
            hints
                .pair("up", "down", "choose")
                .one("confirm", "read it")
                .one("close", "cancel");
        }
        Popup::SaveRequest(dialog) if dialog.picking => {
            hints
                .pair("up", "down", "choose")
                .one("confirm", "pick")
                .one("close", "back");
        }
        Popup::CollectionSettings { .. } => {
            hints
                .pair("up", "down", "choose")
                .one("confirm", "do it")
                .one("close", "close");
        }
        Popup::Auth(dialog) if dialog.row == 0 => {
            hints
                .one("kind", "type")
                .pair("up", "down", "move")
                .one("confirm", "save")
                .one("close", "cancel");
        }
        Popup::Auth(_) => {
            hints
                .pair("up", "down", "move")
                .one("confirm", "save")
                .one("close", "cancel");
        }
        Popup::Run => {
            hints.one("close", "close");
        }
        Popup::Workflows { .. } => {
            hints
                .pair("up", "down", "choose")
                .one("confirm", "run")
                .one("edit", "edit")
                .one("row.delete", "delete")
                .one("close", "close");
        }
        Popup::WorkflowEditor(editor) if editor.picking.is_some() => {
            hints
                .pair("up", "down", "choose")
                .one("confirm", "pick")
                .one("close", "back");
        }
        Popup::WorkflowEditor(editor) => {
            hints.pair("up", "down", "move");

            if matches!(editor.current(), crate::app::WorkflowRow::Capture(..)) {
                hints.one("next-cell", "next field");
            }

            hints.one("confirm", "choose");

            if matches!(
                editor.current(),
                crate::app::WorkflowRow::Capture(..) | crate::app::WorkflowRow::Request(_)
            ) {
                hints.one("row.delete", "delete");
            }

            hints.one("save", "save").one("close", "close");
        }
        Popup::SaveRequest(_) => {
            hints
                .one("confirm", "save")
                .one("tab.next", "where")
                .one("close", "cancel");
        }
        Popup::SpecPreview(_) => {
            hints
                .one("confirm", "import")
                .pair("up", "down", "scroll")
                .one("close", "cancel");
        }
        _ => {}
    }
}

pub(super) fn hints(state: &State, popup: &Popup) -> Vec<Hint> {
    let mut hints = Hints::here(state);

    match popup {
        Popup::Prompt { .. } | Popup::Passphrase { .. } => {
            hints.one("confirm", "ok").one("close", "cancel");
        }
        Popup::Confirm { .. } => {
            hints.one("confirm", "yes").one("close", "no");
        }
        Popup::Trust { .. } => {
            hints.one("confirm", "trust").one("close", "not now").range(
                "trust.moved-1",
                "trust.moved-9",
                "moved from",
            );
        }
        Popup::ChooseBackend => {
            hints
                .one("backend.keychain", "keychain")
                .one("backend.file", "encrypted file")
                .one("close", "later");
        }
        Popup::Approve(_) => {
            hints.one("confirm", "send").one("close", "don't send");
        }
        Popup::Settings => {
            hints
                .one("keys", "keys")
                .one("theme", "theme")
                .pair("narrower", "wider", "sidebar")
                .one("migrate", "move secrets")
                .one("close", "close");
        }
        Popup::Welcome { step, .. } => {
            let next = if *step == Step::Keys { "next" } else { "done" };

            hints
                .pair("up", "down", "choose")
                .one("confirm", next)
                .one("close", "keep these");
        }
        Popup::Tabs { .. } => {
            hints
                .range("request.go-1", "request.go-last", "go to")
                .pair("up", "down", "choose")
                .one("confirm", "open")
                .one("request.new", "new")
                .one("request.close", "close")
                .one("close", "back");
        }
        Popup::Environments { .. } => {
            hints
                .pair("up", "down", "choose")
                .one("confirm", "use")
                .one("env.new", "new")
                .one("env.rename", "rename")
                .one("env.delete", "delete")
                .one("env.variables", "variables")
                .one("close", "close");
        }
        Popup::Help { .. } => {
            hints.pair("up", "down", "scroll").one("close", "close");
        }
        Popup::Palette(_) => {
            hints
                .pair("up", "down", "choose")
                .one("confirm", "run")
                .one("close", "close");
        }
        Popup::Variables(editor) if editor.editing() => {
            if let Some((column, _)) = &editor.table().editing {
                super::cell_hints(&mut hints, *column);
            }
        }
        Popup::Variables(_) => {
            hints
                .pair("tab.previous", "tab.next", "tab")
                .one("row.add", "add")
                .one("edit", "edit")
                .one("row.toggle", "on/off")
                .one("make-secret", "secret")
                .one("row.delete", "delete");
        }
        Popup::SaveVariables { .. }
        | Popup::Review(_)
        | Popup::Fill(_)
        | Popup::SpecChoice { .. }
        | Popup::SaveRequest(_)
        | Popup::CollectionSettings { .. }
        | Popup::Auth(_)
        | Popup::Run
        | Popup::Workflows { .. }
        | Popup::WorkflowEditor(_)
        | Popup::SpecPreview(_) => import_hints(&mut hints, popup),
    }

    hints.finish()
}
