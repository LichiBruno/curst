//! the screens shown on the website, drawn by the real interface and
//! written into `site/public/index.html` between `<!-- frame:NAME -->` and
//! `<!-- /frame -->`; run with `cargo test -p curst site_frames -- --ignored`

use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::Duration;

use curst_core::http::{Response, ResponseBody, Timings};
use curst_core::model::{
    Auth, Collection, EnvId, Environment, Folder, Item, KeyValue, LocalId, Method, Request,
    SavedRequest, Variable,
};
use curst_core::vars::VarName;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::{Color, Modifier, Style};
use unicode_width::UnicodeWidthStr;

use super::view;
use crate::app::{Action, Focus, NodeKey, State, Workspace, action_for, update};

const ORDERS: &str = r#"{"items":[{"id":"ord_7Hq2","status":"paid","total":129.5,"currency":"EUR","customer":{"id":"cus_41","email":"ada@example.com"},"lines":3},{"id":"ord_7Hq1","status":"paid","total":42,"currency":"EUR","customer":{"id":"cus_17","email":"lin@example.com"},"lines":1}],"next":"ord_7Hq0"}"#;

fn id(text: &str) -> LocalId {
    LocalId::new(text).unwrap()
}

fn name(text: &str) -> VarName {
    VarName::new(text).unwrap()
}

fn request(slug: &str, name: &str, method: Method, url: &str) -> Item {
    Item::Request(SavedRequest {
        slug: slug.into(),
        name: name.into(),
        request: Request {
            method,
            url: url.into(),
            ..Request::default()
        },
        spec: None,
    })
}

fn folder(slug: &str, name: &str, items: Vec<Item>) -> Item {
    Item::Folder(Folder {
        slug: slug.into(),
        name: name.into(),
        auth: Auth::Inherit,
        items,
    })
}

fn environment(id: &str, name: &str, base_url: &str) -> Environment {
    Environment {
        id: EnvId::new(id).unwrap(),
        slug: id.into(),
        name: name.into(),
        variables: vec![
            Variable::new(self::name("base_url"), base_url),
            Variable::secret(self::name("token")),
        ],
    }
}

fn shop() -> Workspace {
    let mut list = request(
        "list-orders",
        "List orders",
        Method::Get,
        "{{base_url}}/orders",
    );

    if let Item::Request(saved) = &mut list {
        saved.request.params = vec![KeyValue::new("status", "paid"), KeyValue::new("limit", "2")];
        saved.request.headers = vec![KeyValue::new("Accept", "application/json")];
    }

    let mut log_in = request("log-in", "Log in", Method::Post, "{{base_url}}/auth/login");

    if let Item::Request(saved) = &mut log_in {
        saved.request.auth = Auth::None;
    }

    Workspace {
        id: id("shop"),
        root: PathBuf::from("/shop"),
        collection: Collection {
            name: "Shop".into(),
            variables: vec![],
            items: vec![
                log_in,
                folder(
                    "orders",
                    "orders",
                    vec![
                        list,
                        request(
                            "get-order",
                            "Get order",
                            Method::Get,
                            "{{base_url}}/orders/{{id}}",
                        ),
                        request(
                            "create-order",
                            "Create order",
                            Method::Post,
                            "{{base_url}}/orders",
                        ),
                        request(
                            "cancel-order",
                            "Cancel order",
                            Method::Delete,
                            "{{base_url}}/orders/{{id}}",
                        ),
                    ],
                ),
                folder(
                    "products",
                    "products",
                    vec![
                        request(
                            "search",
                            "Search products",
                            Method::Get,
                            "{{base_url}}/products",
                        ),
                        request(
                            "update",
                            "Update product",
                            Method::Put,
                            "{{base_url}}/products/{{id}}",
                        ),
                    ],
                ),
                folder("customers", "customers", vec![]),
            ],
            environments: vec![
                environment("staging", "staging", "https://api.staging.shop.dev"),
                environment("production", "production", "https://api.shop.dev"),
            ],
            source: None,
            auth: Auth::Bearer {
                token: "{{token}}".into(),
            },
            workflows: vec![],
        },
        active_environment: EnvId::new("staging"),
        trusted: true,
        auto_save: false,
        managed: true,
    }
}

fn key(path: &[&str]) -> NodeKey {
    NodeKey {
        collection: id("shop"),
        path: path.iter().map(|p| (*p).to_owned()).collect(),
    }
}

fn loaded(workspaces: Vec<Workspace>) -> State {
    let mut state = State::default();

    update(
        &mut state,
        Action::Loaded {
            collections: workspaces,
            globals: vec![],
            problems: vec![],
            orphans: vec![],
        },
    );
    state.sidebar.expanded.insert(key(&[]));

    state
}

fn press(state: &mut State, event: KeyEvent) {
    for action in action_for(state, event) {
        update(state, action);
    }
}

fn type_text(state: &mut State, text: &str) {
    for c in text.chars() {
        press(state, KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
}

fn answer(body: &str, total: u64) -> Response {
    Response {
        status: 200,
        status_line: "HTTP/2 200".into(),
        headers: vec![("content-type".into(), "application/json".into())],
        body: ResponseBody::from_bytes(body.as_bytes().to_vec()),
        timings: Timings {
            dns: Duration::from_millis(4),
            connect: Duration::from_millis(18),
            tls: Duration::from_millis(31),
            first_byte: Duration::from_millis(total - 9),
            total: Duration::from_millis(total),
        },
        effective_url: "https://api.staging.shop.dev/orders".into(),
        redirects: 0,
        stopped: None,
    }
}

/// the shop collection with "List orders" open and answered
fn hero() -> State {
    let mut state = loaded(vec![shop()]);

    state.sidebar.expanded.insert(key(&["orders"]));
    state.sidebar.expanded.insert(key(&["products"]));
    state.sidebar.selected = Some(key(&["orders", "list-orders"]));
    state.focus = Focus::Sidebar;
    update(&mut state, Action::Edit);
    state.secret_values.insert(
        curst_core::secrets::SecretKey::Environment {
            collection: id("shop"),
            environment: EnvId::new("staging").unwrap(),
            name: name("token"),
        },
        Some(curst_core::secrets::Secret::new("t")),
    );

    let commands = update(&mut state, Action::Send);
    let Some(crate::app::Command::Send { id, .. }) = commands.last() else {
        panic!("not sent: {commands:?}");
    };
    let id = *id;

    update(&mut state, Action::responded(id, Ok(answer(ORDERS, 142))));
    state.focus = Focus::Response;

    state
}

fn token() -> curst_core::secrets::SecretKey {
    curst_core::secrets::SecretKey::Environment {
        collection: id("shop"),
        environment: EnvId::new("production").unwrap(),
        name: name("token"),
    }
}

/// production is chosen, and its token was never sent there
fn approve() -> State {
    let mut state = hero();

    state.collections[0].active_environment = EnvId::new("production");
    state
        .secret_values
        .insert(token(), Some(curst_core::secrets::Secret::new("t")));

    let commands = update(&mut state, Action::Send);
    let Some(crate::app::Command::Send { id, .. }) = commands.last() else {
        panic!("not sent: {commands:?}");
    };
    let id = *id;

    update(
        &mut state,
        Action::NeedsApproval {
            id,
            approval: Box::new(curst_core::secrets::Approval {
                origin: "https://api.shop.dev".into(),
                keys: vec![token()],
            }),
        },
    );

    state
}

fn import() -> State {
    let text = include_str!("../../../curst-core/src/openapi/fixtures/store-3.0.json");
    let converted =
        curst_core::openapi::convert(&curst_core::openapi::parse(text).unwrap(), None).unwrap();
    let preview = crate::app::SpecPreview::new(
        converted,
        curst_core::model::Source {
            location: "https://api.shop.dev/docs".into(),
            ..curst_core::model::Source::default()
        },
    );
    let mut state = loaded(vec![shop()]);

    update(
        &mut state,
        Action::SpecRead(Ok(crate::app::SpecFound::Preview(Box::new(preview)))),
    );

    state
}

fn workflow() -> State {
    use curst_core::model::{Capture, Extract, Step, Workflow};

    let mut workspace = shop();

    workspace.collection.workflows = vec![Workflow {
        name: "Log in".into(),
        steps: vec![
            Step {
                request: vec!["log-in".into()],
                keep: vec![Capture {
                    variable: name("token"),
                    from: Extract::Json("$.access_token".into()),
                }],
            },
            Step {
                request: vec!["orders".into(), "list-orders".into()],
                keep: vec![],
            },
        ],
    }];

    let mut state = hero();

    state.collections[0] = workspace;

    let commands = update(
        &mut state,
        Action::RunWorkflow {
            collection: id("shop"),
            index: 0,
        },
    );
    let Some(crate::app::Command::SendStep { id, .. }) = commands.last() else {
        panic!("no step sent: {commands:?}");
    };
    let id = *id;

    update(
        &mut state,
        Action::StepAnswered {
            id,
            result: Box::new(Ok(answer(
                r#"{"access_token":"eyJhbGciOi","expires_in":3600}"#,
                88,
            ))),
        },
    );

    state
}

fn palette() -> State {
    let mut state = loaded(vec![shop()]);

    state.sidebar.expanded.insert(key(&["orders"]));
    press(
        &mut state,
        KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL),
    );
    type_text(&mut state, "orders");

    state
}

fn vim() -> State {
    let mut state = hero();

    state.keymap = crate::app::Keymap::vim();
    state.keys_preset = "vim".into();
    type_text(&mut state, ":wq");

    state
}

fn buffer(state: &State, width: u16, height: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();

    terminal.draw(|frame| view(state, frame)).unwrap();

    terminal.backend().buffer().clone()
}

fn color(color: Color) -> Option<String> {
    let name = match color {
        Color::Reset => return None,
        Color::Black => "black".into(),
        Color::Red | Color::LightRed => "red".into(),
        Color::Green | Color::LightGreen => "green".into(),
        Color::Yellow | Color::LightYellow => "yellow".into(),
        Color::Blue | Color::LightBlue => "blue".into(),
        Color::Magenta | Color::LightMagenta => "magenta".into(),
        Color::Cyan | Color::LightCyan => "cyan".into(),
        Color::Gray | Color::White => "white".into(),
        Color::DarkGray => "gray".into(),
        Color::Indexed(n) => format!("i{n}"),
        Color::Rgb(r, g, b) => format!("x{r:02x}{g:02x}{b:02x}"),
    };

    Some(name)
}

/// the classes for a style: `f-` the text color, `k-` the background,
/// with reversed resolved here so the page needs no rule for it
fn classes(style: Style) -> String {
    let mut fg = style.fg.and_then(color);
    let mut bg = style.bg.and_then(color);

    if style.add_modifier.contains(Modifier::REVERSED) {
        let swapped_fg = bg.take().unwrap_or_else(|| "screen".into());

        bg = Some(fg.take().unwrap_or_else(|| "ink".into()));
        fg = Some(swapped_fg);
    }

    let mut out = Vec::new();

    if let Some(fg) = fg {
        out.push(format!("f-{fg}"));
    }
    if let Some(bg) = bg {
        out.push(format!("k-{bg}"));
    }
    for (modifier, class) in [
        (Modifier::BOLD, "b"),
        (Modifier::DIM, "d"),
        (Modifier::ITALIC, "i"),
        (Modifier::UNDERLINED, "u"),
    ] {
        if style.add_modifier.contains(modifier) {
            out.push(class.to_owned());
        }
    }

    out.join(" ")
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn html(buffer: &Buffer) -> String {
    let area = buffer.area;
    let mut out = String::new();

    for y in 0..area.height {
        let mut runs: Vec<(String, String)> = Vec::new();
        let mut skip = 0;

        for x in 0..area.width {
            if skip > 0 {
                skip -= 1;

                continue;
            }

            let cell = &buffer[(x, y)];
            let symbol = cell.symbol();

            skip = symbol.width().saturating_sub(1);

            let class = classes(cell.style());

            match runs.last_mut() {
                Some((last, text)) if *last == class => text.push_str(symbol),
                _ => runs.push((class, symbol.to_owned())),
            }
        }

        if let Some((class, text)) = runs.last_mut()
            && class.is_empty()
        {
            *text = text.trim_end().to_owned();
        }

        for (class, text) in runs {
            if class.is_empty() {
                out.push_str(&escape(&text));
            } else {
                let _ = write!(out, "<span class=\"{class}\">{}</span>", escape(&text));
            }
        }

        out.push('\n');
    }

    out.trim_end_matches('\n').to_owned()
}

fn plain(buffer: &Buffer) -> String {
    let area = buffer.area;
    let mut out = String::new();

    for y in 0..area.height {
        let mut skip = 0;

        for x in 0..area.width {
            if skip > 0 {
                skip -= 1;

                continue;
            }

            let symbol = buffer[(x, y)].symbol();

            skip = symbol.width().saturating_sub(1);
            out.push_str(symbol);
        }

        out.push('\n');
    }

    out
}

fn page() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../site/public/index.html")
}

/// draws the screen and puts it in the page between its markers
fn frame(name: &str, label: &str, state: &State, width: u16, height: u16) {
    let buffer = buffer(state, width, height);

    println!("{name}:\n{}", plain(&buffer));

    let path = page();
    let text = std::fs::read_to_string(&path).unwrap();
    let open = format!("<!-- frame:{name} -->");
    let close = "<!-- /frame -->";
    let start = text
        .find(&open)
        .unwrap_or_else(|| panic!("no {open} in the page"))
        + open.len();
    let end = start + text[start..].find(close).unwrap();
    let content = format!(
        "<pre class=\"term\" style=\"--cols:{width}\" role=\"img\" aria-label=\"{label}\">{}</pre>",
        html(&buffer)
    );
    let updated = format!("{}{content}{}", &text[..start], &text[end..]);

    std::fs::write(&path, updated).unwrap();
}

#[test]
#[ignore = "writes the website's screens"]
fn site_frames() {
    frame(
        "hero",
        "curst, a Rust TUI API client, with a Shop collection in the sidebar and a GET request answered with JSON",
        &hero(),
        104,
        26,
    );
    frame(
        "approve",
        "curst asking before it sends a secret token to a new host",
        &approve(),
        104,
        20,
    );
    frame(
        "import",
        "curst previewing an OpenAPI spec before importing it as a collection",
        &import(),
        104,
        18,
    );
    frame(
        "workflow",
        "a curst workflow logging in and keeping the access token for the next request",
        &workflow(),
        104,
        16,
    );
    frame(
        "palette",
        "the curst command palette finding saved requests",
        &palette(),
        104,
        16,
    );
    frame(
        "vim",
        "curst with the vim keymap and its command line",
        &vim(),
        104,
        14,
    );
}
