use proptest::prelude::*;

use super::*;

fn request(command: &str) -> Request {
    parse(command).unwrap().request
}

#[test]
fn a_browser_copy_becomes_a_request() {
    // the shape Chrome and Firefox copy, minus most of their headers
    let command = r#"curl 'https://api.example.com/v1/items?page=2&q=red%20shoes' \
  -H 'accept: application/json' \
  -H 'content-type: application/json' \
  -H 'user-agent: Mozilla/5.0' \
  --data-raw '{"name":"box"}' \
  --compressed"#;
    let parsed = parse(command).unwrap();

    assert_eq!(parsed.warnings, Vec::<String>::new());
    assert_eq!(
        parsed.request,
        Request {
            method: Method::Post,
            url: "https://api.example.com/v1/items".into(),
            params: vec![KeyValue::new("page", "2"), KeyValue::new("q", "red shoes")],
            headers: vec![
                KeyValue::new("accept", "application/json"),
                KeyValue::new("user-agent", "Mozilla/5.0"),
            ],
            body: Body::Json(r#"{"name":"box"}"#.into()),
            auth: Auth::None,
        }
    );
}

#[test]
fn options_are_read_however_they_are_written() {
    let forms = [
        "curl -XPUT https://h/x",
        "curl -X PUT https://h/x",
        "curl --request=PUT https://h/x",
        "curl --request PUT -sSL https://h/x",
        "curl https://h/x -sSXPUT",
    ];

    for form in forms {
        assert_eq!(request(form).method, Method::Put, "{form}");
        assert_eq!(request(form).url, "https://h/x", "{form}");
    }

    // a value that starts with a dash is still a value
    assert_eq!(
        request("curl -H '-x-odd: 1' https://h").headers,
        [KeyValue::new("-x-odd", "1")]
    );
    assert_eq!(request("curl -I https://h").method, Method::Head);
    assert_eq!(request("curl -H 'X-Empty;' https://h").headers[0].value, "");
}

#[test]
fn data_picks_the_body_kind() {
    assert_eq!(
        request("curl -d a=1 -d 'b=two%20words' https://h").body,
        Body::Form(vec![
            KeyValue::new("a", "1"),
            KeyValue::new("b", "two words")
        ])
    );
    assert_eq!(request("curl -d a=1 https://h").method, Method::Post);
    assert_eq!(
        request(r#"curl -d '{"a":1}' https://h"#).body,
        Body::Json(r#"{"a":1}"#.into())
    );
    assert_eq!(
        request(r#"curl --json '{"a":1}' https://h"#).body,
        Body::Json(r#"{"a":1}"#.into())
    );
    assert_eq!(
        request("curl -H 'Content-Type: text/csv' -d 'a,b' https://h").body,
        Body::Text {
            content_type: "text/csv".into(),
            text: "a,b".into()
        }
    );
    assert_eq!(
        request("curl --data-urlencode 'q=a b&c' https://h").body,
        Body::Form(vec![KeyValue::new("q", "a b&c")])
    );

    let get = request("curl -G -d q=rust https://h/s");

    assert_eq!((get.method, get.body), (Method::Get, Body::None));
    assert_eq!(get.params, [KeyValue::new("q", "rust")]);
}

#[test]
fn auth_takes_its_own_place() {
    assert_eq!(
        request("curl -u ann:pw https://h").auth,
        Auth::Basic {
            username: "ann".into(),
            password: "pw".into()
        }
    );
    assert_eq!(
        request("curl https://ann:p%40ss@h/x").auth,
        Auth::Basic {
            username: "ann".into(),
            password: "p@ss".into()
        }
    );
    assert_eq!(request("curl https://ann:pw@h/x").url, "https://h/x");

    let bearer = request("curl -H 'Authorization: Bearer abc' https://h");

    assert_eq!(
        bearer.auth,
        Auth::Bearer {
            token: "abc".into()
        }
    );
    assert_eq!(bearer.headers.len(), 0);

    // Basic with base64 user and password: YW5uOnB3 is ann:pw
    assert_eq!(
        request("curl -H 'Authorization: Basic YW5uOnB3' https://h").auth,
        Auth::Basic {
            username: "ann".into(),
            password: "pw".into()
        }
    );
    // an Authorization header is what curl sends, over -u
    let both = parse("curl -u ann:pw -H 'Authorization: Bearer abc' https://h").unwrap();

    assert_eq!(
        both.request.auth,
        Auth::Bearer {
            token: "abc".into()
        }
    );
    assert_eq!(both.warnings.len(), 1);
    assert_eq!(
        request("curl -b 'sid=1; theme=dark' https://h").headers,
        [KeyValue::new("Cookie", "sid=1; theme=dark")]
    );
}

#[test]
fn files_are_never_read_and_the_rest_is_reported() {
    let parsed = parse(
        "curl -d @secret.json -b cookies.txt -F file=@a.png -K cfg -T up -x proxy:1 -k -Z https://h",
    )
    .unwrap();

    assert_eq!(parsed.request.body, Body::None);
    assert_eq!(parsed.request.method, Method::Get);
    assert_eq!(parsed.warnings.len(), 8, "{:?}", parsed.warnings);
    assert!(parsed.warnings[0].contains("does not read files"));
}

#[test]
fn what_is_not_a_command_says_why() {
    assert_eq!(parse("wget https://h"), Err(CurlError::NotCurl));
    assert_eq!(parse(""), Err(CurlError::NotCurl));
    assert_eq!(parse("curl -s"), Err(CurlError::NoUrl));
    assert_eq!(
        parse("curl https://h -H"),
        Err(CurlError::MissingValue("-H".into()))
    );
    assert!(matches!(parse("curl 'https://h"), Err(CurlError::Shell(_))));
    assert_eq!(request("/usr/bin/curl https://h").url, "https://h");
}

#[test]
fn emitted_commands_keep_placeholders_and_never_read_files() {
    let request = Request {
        method: Method::Post,
        url: "https://h/{{version}}/x".into(),
        params: vec![
            KeyValue::new("key", "{{api_key}}"),
            KeyValue {
                name: "off".into(),
                value: "1".into(),
                enabled: false,
            },
        ],
        headers: vec![KeyValue::new("X-Id", "it's")],
        body: Body::Text {
            content_type: "text/plain".into(),
            text: "@not-a-file".into(),
        },
        auth: Auth::Bearer {
            token: "{{token}}".into(),
        },
    };
    let command = emit(&request);

    assert_eq!(
        command,
        "curl 'https://h/{{version}}/x?key={{api_key}}' \\\n  -X POST \\\n  -H 'X-Id: it'\\''s' \\\n  -H 'Authorization: Bearer {{token}}' \\\n  -H 'Content-Type: text/plain' \\\n  --data-raw @not-a-file"
    );
    assert_eq!(emit(&Request::default()), "curl ''");

    // what only looks like a variable is encoded like any text
    let odd = Request {
        url: "https://h".into(),
        params: vec![KeyValue::new("a", "{{&}}"), KeyValue::new("b", "{{ok}}")],
        ..Request::default()
    };

    assert_eq!(emit(&odd), "curl 'https://h?a=%7B%7B%26%7D%7D&b={{ok}}'");
    assert_eq!(parse(&emit(&odd)).unwrap().request.params, odd.params);
}

#[test]
fn credentials_become_placeholders() {
    let command = "curl 'https://h/x?api_key=k123&page=1' -u ann:pw \
                   -H 'Cookie: sid=s3cr3t' -H 'X-Api-Key: xyz'";
    let imported = import(command, |name| name == "api_key").unwrap();
    let values: Vec<(&str, &str)> = imported
        .credentials
        .iter()
        .map(|c| (c.name.as_str(), c.value.expose()))
        .collect();

    // `api_key` is taken where they will be kept, so the next name is used
    assert_eq!(
        values,
        [
            ("api_key_2", "k123"),
            ("cookie", "sid=s3cr3t"),
            ("x_api_key", "xyz"),
            ("password", "pw"),
        ]
    );
    assert_eq!(imported.request.params[0].value, "{{api_key_2}}");
    assert_eq!(imported.request.params[1].value, "1");
    assert_eq!(
        imported.request.auth,
        Auth::Basic {
            username: "ann".into(),
            password: "{{password}}".into()
        }
    );

    // nothing of a value is left in the request
    let text = format!("{:?}", imported.request);

    for secret in ["k123", "s3cr3t", "xyz", "pw\""] {
        assert!(!text.contains(secret), "{secret} in {text}");
    }

    let header = import(
        "curl -H 'Proxy-Authorization: Bearer t0k' https://h",
        |_| false,
    )
    .unwrap();

    assert_eq!(
        header.request.headers[0].value,
        "Bearer {{proxy_authorization}}"
    );
    assert_eq!(header.credentials[0].value.expose(), "t0k");
}

#[test]
fn names_come_from_what_they_were_called() {
    assert_eq!(name_for("X-Api-Key"), "x_api_key");
    assert_eq!(name_for("  "), "secret");
    assert_eq!(split_scheme("Bearer abc"), ("Bearer ", "abc"));
    assert_eq!(split_scheme("abc"), ("", "abc"));
}

/// text for names and values: printable, with the characters that need
/// quoting or encoding
fn text() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9 '\"$\\\\&=?#%+@{}é-]{0,12}"
}

fn name() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9_]{0,8}"
}

fn pairs() -> impl Strategy<Value = Vec<KeyValue>> {
    prop::collection::vec((name(), text()), 0..4).prop_map(|pairs| {
        pairs
            .into_iter()
            .map(|(n, v)| KeyValue::new(n, v))
            .collect()
    })
}

/// a request as `parse` gives it back: every row on, the query in the
/// parameters, `Authorization` in the auth and the content type in the
/// body's kind
fn canonical() -> impl Strategy<Value = Request> {
    let method = prop::sample::select(vec![
        Method::Get,
        Method::Post,
        Method::Put,
        Method::Patch,
        Method::Delete,
        Method::Head,
        Method::Options,
    ]);
    let headers = prop::collection::vec(("X-[A-Za-z]{1,8}", "[!-~]([ -~]{0,10}[!-~])?"), 0..3)
        .prop_map(|h| {
            h.into_iter()
                .map(|(n, v)| KeyValue::new(n, v))
                .collect::<Vec<_>>()
        });
    let body = prop_oneof![
        Just(Body::None),
        "[ -~\n]{1,20}".prop_map(Body::Json),
        "[ -~\n]{1,20}".prop_map(|text| Body::Text {
            content_type: "text/plain".into(),
            text,
        }),
        prop::collection::vec((name(), text()), 1..4).prop_map(|pairs| Body::Form(
            pairs
                .into_iter()
                .map(|(n, v)| KeyValue::new(n, v))
                .collect()
        )),
    ];
    let auth = prop_oneof![
        Just(Auth::None),
        ("[a-z]{1,8}", "[ -~]{0,12}")
            .prop_map(|(username, password)| Auth::Basic { username, password }),
        "[A-Za-z0-9._{}-]{1,20}".prop_map(|token| Auth::Bearer { token }),
    ];

    (method, "[a-z]{0,8}", pairs(), headers, body, auth).prop_map(
        |(method, path, params, headers, body, auth)| Request {
            body: if method == Method::Head {
                Body::None
            } else {
                body
            },
            method,
            url: format!("https://example.com/{path}"),
            params,
            headers,
            auth,
        },
    )
}

proptest! {
    #[test]
    fn a_request_comes_back_from_its_command(request in canonical()) {
        let parsed = parse(&emit(&request)).unwrap();

        prop_assert_eq!(parsed.warnings, Vec::<String>::new());
        prop_assert_eq!(parsed.request, request);
    }

    #[test]
    fn any_text_is_read_without_a_panic(command in "curl [ -~]{0,60}") {
        let _ = parse(&command);
    }
}
