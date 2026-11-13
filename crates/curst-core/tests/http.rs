#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use curst_core::http::{
    Cancel, Config, CurlExecutor, HttpError, HttpExecutor, RedirectRule, Response,
};
use curst_core::model::{Auth, Body, KeyValue, Method, Request};
use httpmock::MockServer;

fn send(request: &Request) -> Result<Response, HttpError> {
    CurlExecutor::default().execute(request, &Cancel::new(), &RedirectRule::Any)
}

fn send_with(config: Config, request: &Request) -> Result<Response, HttpError> {
    CurlExecutor::new(config).execute(request, &Cancel::new(), &RedirectRule::Any)
}

/// a plain HTTP URL on the mock server, which answers HTTP and HTTPS on the
/// same port
fn http(server: &MockServer, path: &str) -> String {
    format!("http://{}{path}", server.address())
}

fn https(server: &MockServer, path: &str) -> String {
    format!("https://localhost:{}{path}", server.port())
}

fn mock_ca() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/httpmock-ca.pem")
}

fn get(url: String) -> Request {
    Request {
        url,
        ..Request::default()
    }
}

#[test]
fn every_method_reaches_the_server() {
    let server = MockServer::start();

    for method in Method::ALL {
        let mut mock = server.mock(|when, then| {
            when.method(method.as_str()).path("/m");
            then.status(204);
        });

        let response = send(&Request {
            method,
            ..get(http(&server, "/m"))
        })
        .unwrap();

        assert_eq!(response.status, 204, "{method}");
        mock.assert();

        mock.delete();
    }
}

#[test]
fn headers_params_and_default_user_agent_are_sent() {
    let server = MockServer::start();
    let mut disabled = KeyValue::new("X-Off", "1");
    disabled.enabled = false;

    let mock = server.mock(|when, then| {
        when.method("GET")
            .path("/search")
            .query_param("q", "a b")
            .query_param("page", "2")
            .header("x-trace", "abc")
            .header("user-agent", concat!("curst/", env!("CARGO_PKG_VERSION")))
            .header_missing("x-off");

        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"ok":true}"#);
    });

    let response = send(&Request {
        params: vec![KeyValue::new("q", "a b"), KeyValue::new("page", "2")],
        headers: vec![KeyValue::new("X-Trace", "abc"), disabled],
        ..get(http(&server, "/search"))
    })
    .unwrap();

    mock.assert();
    assert_eq!(response.header("content-type"), Some("application/json"));
    assert_eq!(response.body.preview(), br#"{"ok":true}"#);
    assert!(response.status_line.starts_with("HTTP/1.1 200"));
    assert!(response.timings.total >= response.timings.first_byte);
    assert_eq!(response.redirects, 0);
}

#[test]
fn user_headers_override_defaults() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.header("user-agent", "mine")
            .header("content-type", "application/vnd.api+json");
        then.status(200);
    });

    send(&Request {
        method: Method::Post,
        headers: vec![
            KeyValue::new("User-Agent", "mine"),
            KeyValue::new("Content-Type", "application/vnd.api+json"),
        ],
        body: Body::Json("{}".into()),
        ..get(http(&server, "/"))
    })
    .unwrap();

    mock.assert();
}

#[test]
fn json_and_form_bodies() {
    let server = MockServer::start();
    let json = server.mock(|when, then| {
        when.method("PUT")
            .path("/json")
            .header("content-type", "application/json")
            .json_body(serde_json::json!({"name": "curst"}));
        then.status(200);
    });

    let form = server.mock(|when, then| {
        when.method("POST")
            .path("/form")
            .header("content-type", "application/x-www-form-urlencoded")
            .form_urlencoded_tuple("a", "1 2")
            .form_urlencoded_tuple("b", "&");
        then.status(200);
    });

    send(&Request {
        method: Method::Put,
        body: Body::Json(r#"{"name":"curst"}"#.into()),
        ..get(http(&server, "/json"))
    })
    .unwrap();

    send(&Request {
        method: Method::Post,
        body: Body::Form(vec![KeyValue::new("a", "1 2"), KeyValue::new("b", "&")]),
        ..get(http(&server, "/form"))
    })
    .unwrap();

    json.assert();
    form.assert();
}

#[test]
fn large_post_body_does_not_wait_for_100_continue() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method("POST").path("/big").header_missing("expect");
        then.status(200);
    });

    send(&Request {
        method: Method::Post,
        body: Body::Text {
            content_type: "text/plain".into(),
            text: "x".repeat(64 * 1024),
        },
        ..get(http(&server, "/big"))
    })
    .unwrap();

    mock.assert();
}

#[test]
fn basic_and_bearer_auth() {
    let server = MockServer::start();
    let basic = server.mock(|when, then| {
        when.path("/basic")
            .header("authorization", "Basic dXNlcjpwYXNz");
        then.status(200);
    });

    let bearer = server.mock(|when, then| {
        when.path("/bearer").header("authorization", "Bearer t0k");
        then.status(200);
    });

    send(&Request {
        auth: Auth::Basic {
            username: "user".into(),
            password: "pass".into(),
        },
        ..get(http(&server, "/basic"))
    })
    .unwrap();

    send(&Request {
        auth: Auth::Bearer {
            token: "t0k".into(),
        },
        ..get(http(&server, "/bearer"))
    })
    .unwrap();

    basic.assert();
    bearer.assert();
}

#[test]
fn redirects_are_followed_and_only_final_headers_kept() {
    let server = MockServer::start();

    server.mock(|when, then| {
        when.path("/old");

        then.status(301)
            .header("location", "/new")
            .header("x-hop", "1");
    });

    server.mock(|when, then| {
        when.path("/new");
        then.status(200).header("x-final", "1").body("done");
    });

    let response = send(&get(http(&server, "/old"))).unwrap();

    assert_eq!(response.status, 200);
    assert_eq!(response.redirects, 1);
    assert!(response.effective_url.ends_with("/new"));
    assert_eq!(response.header("x-final"), Some("1"));
    assert_eq!(response.header("x-hop"), None);
    assert_eq!(response.body.preview(), b"done");
}

#[test]
fn redirects_can_be_turned_off_and_are_limited() {
    let server = MockServer::start();

    server.mock(|when, then| {
        when.path("/loop");
        then.status(302).header("location", "/loop");
    });

    let no_follow = Config {
        follow_redirects: false,
        ..Config::default()
    };

    let response = send_with(no_follow, &get(http(&server, "/loop"))).unwrap();

    assert_eq!(response.status, 302);
    assert_eq!(response.header("location"), Some("/loop"));

    let limited = Config {
        max_redirects: 3,
        ..Config::default()
    };

    let err = send_with(limited, &get(http(&server, "/loop"))).unwrap_err();

    assert!(matches!(err, HttpError::TooManyRedirects), "{err:?}");
}

#[test]
fn timeout() {
    let server = MockServer::start();

    server.mock(|when, then| {
        when.path("/slow");
        then.status(200).delay(Duration::from_secs(5));
    });

    let config = Config {
        timeout: Some(Duration::from_millis(300)),
        ..Config::default()
    };

    let err = send_with(config, &get(http(&server, "/slow"))).unwrap_err();

    assert!(matches!(err, HttpError::Timeout(_)), "{err:?}");
}

#[test]
fn cancel_stops_a_request_in_flight() {
    let server = MockServer::start();

    server.mock(|when, then| {
        when.path("/slow");
        then.status(200).delay(Duration::from_secs(5));
    });

    let cancel = Cancel::new();
    let trigger = cancel.clone();
    let canceler = thread::spawn(move || {
        thread::sleep(Duration::from_millis(200));
        trigger.cancel();
    });

    let started = Instant::now();
    let err = CurlExecutor::default()
        .execute(&get(http(&server, "/slow")), &cancel, &RedirectRule::Any)
        .unwrap_err();
    canceler.join().unwrap();

    assert!(matches!(err, HttpError::Canceled), "{err:?}");
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[test]
fn body_over_the_cap_goes_to_a_temporary_file() {
    let server = MockServer::start();
    let payload = "0123456789".repeat(1000);

    server.mock(|when, then| {
        when.path("/big");
        then.status(200).body(&payload);
    });

    let config = Config {
        body_cap: 100,
        ..Config::default()
    };

    let response = send_with(config, &get(http(&server, "/big"))).unwrap();

    assert!(response.body.is_truncated());
    assert_eq!(response.body.len(), 10_000);
    assert_eq!(response.body.preview(), &payload.as_bytes()[..100]);

    let dir = tempfile::tempdir().unwrap();
    let saved = dir.path().join("body");

    response.body.save_to(&saved).unwrap();

    assert_eq!(std::fs::read(saved).unwrap(), payload.as_bytes());
}

#[test]
fn connection_and_url_errors() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let closed = format!("http://{}/", listener.local_addr().unwrap());
    drop(listener);

    let err = send(&get(closed)).unwrap_err();

    assert!(matches!(err, HttpError::Connect(_)), "{err:?}");

    let err = send(&get("nope://host/".into())).unwrap_err();

    assert!(matches!(err, HttpError::InvalidUrl(_)), "{err:?}");

    let err = send(&get("http://host.invalid/".into())).unwrap_err();

    assert!(matches!(err, HttpError::Resolve(_)), "{err:?}");
}

#[test]
fn bodyless_post_sends_an_empty_body() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method("POST")
            .path("/empty")
            .header("content-length", "0")
            .header_missing("transfer-encoding")
            .header_missing("content-type");
        then.status(201);
    });

    let response = send(&Request {
        method: Method::Post,
        ..get(http(&server, "/empty"))
    })
    .unwrap();

    assert_eq!(response.status, 201);
    mock.assert();
}

#[test]
fn redirects_keep_or_drop_method_and_body_like_curl() {
    let server = MockServer::start();

    server.mock(|when, then| {
        when.path("/see-other");
        then.status(303).header("location", "/target");
    });

    server.mock(|when, then| {
        when.path("/temporary");
        then.status(307).header("location", "/target");
    });

    server.mock(|when, then| {
        when.path("/moved");
        then.status(301).header("location", "/target");
    });

    server.mock(|when, then| {
        when.path("/found");
        then.status(302).header("location", "/target");
    });

    server.mock(|when, then| {
        when.path("/permanent");
        then.status(308).header("location", "/target");
    });

    // 301, 302 and 303 after a POST become a GET without the body; 307
    // and 308 keep both
    let cases = [
        (Method::Post, "/moved", "GET", ""),
        (Method::Post, "/found", "GET", ""),
        (Method::Post, "/see-other", "GET", ""),
        (Method::Put, "/see-other", "GET", ""),
        (Method::Delete, "/see-other", "GET", ""),
        (Method::Post, "/temporary", "POST", "payload"),
        (Method::Put, "/temporary", "PUT", "payload"),
        (Method::Delete, "/temporary", "DELETE", "payload"),
        (Method::Post, "/permanent", "POST", "payload"),
        (Method::Put, "/permanent", "PUT", "payload"),
    ];

    for (method, path, expected_method, expected_body) in cases {
        let mut target = server.mock(|when, then| {
            let when = when.path("/target").method(expected_method);

            if expected_body.is_empty() {
                when.header_missing("content-length");
            } else {
                when.body(expected_body);
            }

            then.status(200);
        });

        let response = send(&Request {
            method,
            body: Body::Text {
                content_type: "text/plain".into(),
                text: "payload".into(),
            },
            ..get(http(&server, path))
        })
        .unwrap();

        assert_eq!(response.status, 200, "{method} {path}");
        target.assert();

        target.delete();
    }
}

#[test]
fn credentials_stay_on_the_same_origin() {
    let server = MockServer::start();

    server.mock(|when, then| {
        when.path("/a");
        then.status(302).header("location", "/b");
    });

    let target = server.mock(|when, then| {
        when.path("/b")
            .header("authorization", "Bearer t0k")
            .header("cookie", "s=1");
        then.status(200);
    });

    send(&Request {
        headers: vec![KeyValue::new("Cookie", "s=1")],
        auth: Auth::Bearer {
            token: "t0k".into(),
        },
        ..get(http(&server, "/a"))
    })
    .unwrap();

    target.assert();
}

#[test]
fn credentials_are_dropped_when_a_redirect_leaves_the_origin() {
    let first = MockServer::start();
    let other = MockServer::start();

    first.mock(|when, then| {
        when.path("/out");
        then.status(307)
            .header("location", http(&other, "/landing"));
    });

    let landing = other.mock(|when, then| {
        when.path("/landing")
            .header_missing("authorization")
            .header_missing("cookie")
            .header("x-trace", "1");
        then.status(200);
    });

    for auth in [
        Auth::Bearer {
            token: "t0k".into(),
        },
        Auth::Basic {
            username: "u".into(),
            password: "p".into(),
        },
    ] {
        let response = send(&Request {
            headers: vec![
                KeyValue::new("Cookie", "s=1"),
                KeyValue::new("X-Trace", "1"),
            ],
            auth,
            ..get(http(&first, "/out"))
        })
        .unwrap();

        assert_eq!(response.status, 200);
        assert_eq!(response.redirects, 1);
        assert_eq!(response.effective_url, http(&other, "/landing"));
    }

    landing.assert_calls(2);
}

#[test]
fn head_stays_head_through_a_303() {
    let server = MockServer::start();

    server.mock(|when, then| {
        when.path("/h");
        then.status(303).header("location", "/i");
    });

    let target = server.mock(|when, then| {
        when.method("HEAD").path("/i");
        then.status(200);
    });

    send(&Request {
        method: Method::Head,
        ..get(http(&server, "/h"))
    })
    .unwrap();

    target.assert();
}

#[test]
fn the_body_cap_applies_after_a_redirect() {
    let server = MockServer::start();

    server.mock(|when, then| {
        when.path("/from");
        then.status(302).header("location", "/to");
    });

    server.mock(|when, then| {
        when.path("/to");
        then.status(200).body("abcdef");
    });

    let config = Config {
        body_cap: 4,
        ..Config::default()
    };

    let response = send_with(config, &get(http(&server, "/from"))).unwrap();

    assert_eq!(response.body.preview(), b"abcd");
    assert_eq!(response.body.len(), 6);
}

const GZIPPED: [u8; 35] = [
    0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0xab, 0x56, 0xaa, 0xca, 0x2c, 0x28,
    0x48, 0x4d, 0x51, 0xb2, 0x2a, 0x29, 0x2a, 0x4d, 0xad, 0x05, 0x00, 0x84, 0x28, 0xbc, 0x08, 0x0f,
    0x00, 0x00, 0x00,
];

#[test]
fn compressed_responses_are_asked_for_and_decoded() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.path("/zip").header_includes("accept-encoding", "gzip");
        then.status(200)
            .header("content-type", "application/json")
            .header("content-encoding", "gzip")
            .body(GZIPPED);
    });

    let response = send(&get(http(&server, "/zip"))).unwrap();

    mock.assert();
    assert_eq!(response.body.preview(), br#"{"zipped":true}"#);
    assert_eq!(response.header("content-encoding"), Some("gzip"));
}

#[test]
fn a_user_accept_encoding_replaces_the_default() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.path("/plain").header("accept-encoding", "identity");
        then.status(200).body("plain");
    });

    let response = send(&Request {
        headers: vec![KeyValue::new("Accept-Encoding", "identity")],
        ..get(http(&server, "/plain"))
    })
    .unwrap();

    mock.assert();
    assert_eq!(response.body.preview(), b"plain");
}

#[test]
fn https_is_checked_against_the_system_store_by_default() {
    let server = MockServer::start();

    let err = send(&get(https(&server, "/secure"))).unwrap_err();

    assert!(matches!(err, HttpError::Tls(_)), "{err:?}");
}

#[test]
fn a_ca_file_is_trusted_instead_of_the_system_store() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.path("/secure");
        then.status(200).body("ok");
    });
    let config = Config {
        ca_file: Some(mock_ca()),
        ..Config::default()
    };

    let response = send_with(config, &get(https(&server, "/secure"))).unwrap();

    mock.assert();
    assert_eq!(response.status, 200);
}

#[test]
fn insecure_skips_the_certificate_check() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.path("/secure");
        then.status(200);
    });
    let config = Config {
        insecure: true,
        ..Config::default()
    };

    let response = send_with(config, &get(https(&server, "/secure"))).unwrap();

    mock.assert();
    assert_eq!(response.status, 200);
}

#[test]
fn a_missing_ca_file_is_a_tls_error() {
    let server = MockServer::start();
    let config = Config {
        ca_file: Some(PathBuf::from("does/not/exist.pem")),
        ..Config::default()
    };

    let err = send_with(config, &get(https(&server, "/secure"))).unwrap_err();

    assert!(matches!(err, HttpError::Tls(_)), "{err:?}");
    assert!(err.to_string().contains("trusted certificates"), "{err}");
}

fn within(urls: &[&str]) -> RedirectRule {
    RedirectRule::Within(
        urls.iter()
            .map(|u| curst_core::url::origin(u).unwrap())
            .collect(),
    )
}

#[test]
fn a_request_with_secrets_follows_redirects_within_approved_origins() {
    let server = MockServer::start();
    let target = http(&server, "/to");

    server.mock(|when, then| {
        when.path("/from");
        then.status(307).header("location", "/to");
    });

    let landed = server.mock(|when, then| {
        when.path("/to");
        then.status(200);
    });
    let response = CurlExecutor::default()
        .execute(
            &get(http(&server, "/from")),
            &Cancel::new(),
            &within(&[&target]),
        )
        .unwrap();

    landed.assert();
    assert_eq!((response.status, response.stopped), (200, None));
}

#[test]
fn a_redirect_to_another_origin_stops_and_says_where() {
    let first = MockServer::start();
    let other = MockServer::start();
    let elsewhere = http(&other, "/landing");

    for (path, status) in [("/temporary", 307), ("/see-other", 303)] {
        first.mock(|when, then| {
            when.path(path);
            then.status(status).header("location", elsewhere.as_str());
        });
    }

    let never = other.mock(|when, then| {
        when.path("/landing");
        then.status(200);
    });
    let post = |path: &str| Request {
        method: Method::Post,
        body: Body::Json("{}".into()),
        ..get(http(&first, path))
    };
    let rule = within(&[&http(&first, "/")]);
    let kept = CurlExecutor::default()
        .execute(&post("/temporary"), &Cancel::new(), &rule)
        .unwrap();
    let dropped = CurlExecutor::default()
        .execute(&post("/see-other"), &Cancel::new(), &rule)
        .unwrap();

    never.assert_calls(0);
    assert_eq!(kept.status, 307);
    assert_eq!(
        kept.stopped,
        Some(curst_core::http::StoppedRedirect {
            url: elsewhere.clone(),
            method: Method::Post,
            keeps_body: true,
        })
    );
    assert_eq!(
        dropped.stopped.map(|s| (s.method, s.keeps_body)),
        Some((Method::Get, false))
    );
}

#[test]
fn a_redirect_from_https_to_http_stops() {
    let server = MockServer::start();
    let plain = format!("http://localhost:{}/plain", server.port());

    server.mock(|when, then| {
        when.path("/secure");
        then.status(307).header("location", plain.as_str());
    });

    let config = Config {
        ca_file: Some(mock_ca()),
        ..Config::default()
    };
    let response = CurlExecutor::new(config)
        .execute(
            &get(https(&server, "/secure")),
            &Cancel::new(),
            &within(&[&https(&server, "/")]),
        )
        .unwrap();

    assert_eq!(response.stopped.map(|s| s.url), Some(plain));
}

#[test]
fn a_swagger_ui_page_leads_to_its_spec_over_http() {
    let server = MockServer::start();
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/openapi/fixtures");
    let read = |name: &str| std::fs::read_to_string(fixtures.join(name)).unwrap();

    server.mock(|when, then| {
        when.path("/docs/");
        then.status(200)
            .header("content-type", "text/html")
            .body(read("swagger-ui.html"));
    });
    server.mock(|when, then| {
        when.path("/docs/swagger-initializer.js");
        then.status(200).body(read("swagger-initializer.js"));
    });

    let spec = server.mock(|when, then| {
        when.path("/v1/openapi.yaml");
        then.status(200).body(read("notes-3.1.yaml"));
    });

    // a redirect on the way is followed, as a browser would
    server.mock(|when, then| {
        when.path("/docs");
        then.status(301).header("location", "/docs/");
    });

    let executor = CurlExecutor::default();
    let cancel = Cancel::new();
    let (value, at) = curst_core::openapi::discover(&http(&server, "/docs"), |url| {
        curst_core::openapi::fetch_with(&executor, url, &[], &cancel, &RedirectRule::Any)
    })
    .unwrap();

    assert_eq!(value["info"]["title"], "Notes");
    assert_eq!(at, http(&server, "/v1/openapi.yaml"));
    spec.assert();

    // a relative server in it is resolved against where it was found
    let converted = curst_core::openapi::convert(&value, Some(&at)).unwrap();
    let base = converted.bundle.environments[0]
        .1
        .iter()
        .find(|v| v.name.as_str() == "base_url")
        .unwrap();

    assert_eq!(base.value, http(&server, "/api"));
}
