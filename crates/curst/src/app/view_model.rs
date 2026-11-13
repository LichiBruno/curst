use std::time::Duration;

use curst_core::http::{BodyKind, Response};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::ResponseTab;

/// a response with the text of every tab prepared once, on arrival, so
/// drawing a frame only slices lines
#[derive(Debug)]
pub(crate) struct ResponseView {
    pub(crate) response: Response,
    pub(crate) summary: String,
    body: Vec<String>,
    headers: Vec<String>,
    timing: Vec<String>,
    raw: Vec<String>,

    /// terminal columns taken by the longest line of each tab, in
    /// [`ResponseTab::ALL`] order
    widths: [usize; 4],

    /// whether the body's last line says which key saves it
    save_note: bool,
}

impl ResponseView {
    pub(crate) fn new(response: Response) -> Self {
        let kind = response.kind();
        let size = format_size(response.body.len());
        let summary = format!(
            "{} · {} · {}",
            status_text(&response),
            format_duration(response.timings.total),
            size
        );

        let body = body_lines(&response, kind);
        let headers: Vec<String> = response
            .headers
            .iter()
            .map(|(name, value)| display(&format!("{name}: {value}")))
            .collect();
        let timing = timing_lines(&response);

        let mut raw = vec![display(&response.status_line)];
        raw.extend(headers.iter().cloned());
        raw.push(String::new());

        if kind == BodyKind::Binary {
            raw.push(format!("<{size} of binary data>"));
        } else {
            raw.extend(text_lines(response.body.preview()));
        }

        let widths = [&body, &headers, &timing, &raw].map(|lines| widest(lines));
        let save_note = kind == BodyKind::Binary || response.body.is_truncated();

        Self {
            response,
            summary,
            body,
            headers,
            timing,
            raw,
            widths,
            save_note,
        }
    }

    /// names the key that saves the body in the note about it, since the
    /// view is prepared away from the keymap
    pub(crate) fn name_save_key(&mut self, key: &str) {
        if !self.save_note {
            return;
        }

        if let Some(line) = self.body.last_mut() {
            *line = line.replacen("press s ", &format!("press {key} "), 1);
        }

        self.widths[0] = widest(&self.body);
    }

    pub(crate) fn lines(&self, tab: ResponseTab) -> &[String] {
        match tab {
            ResponseTab::Body => &self.body,
            ResponseTab::Headers => &self.headers,
            ResponseTab::Timing => &self.timing,
            ResponseTab::Raw => &self.raw,
        }
    }

    pub(crate) fn width(&self, tab: ResponseTab) -> usize {
        let index = ResponseTab::ALL.iter().position(|t| *t == tab);

        index
            .and_then(|i| self.widths.get(i))
            .copied()
            .unwrap_or_default()
    }

    pub(crate) fn extension(&self) -> &'static str {
        match self.response.kind() {
            BodyKind::Json => "json",
            BodyKind::Text => "txt",
            BodyKind::Binary => "bin",
        }
    }
}

fn widest(lines: &[String]) -> usize {
    lines
        .iter()
        .map(|line| line.width())
        .max()
        .unwrap_or_default()
}

/// the rest of `line` once `columns` terminal columns are scrolled past; a
/// wide character cut in half is skipped whole
pub(crate) fn skip_columns(line: &str, columns: usize) -> &str {
    let mut skipped = 0;

    for (index, c) in line.char_indices() {
        if skipped >= columns {
            return &line[index..];
        }

        skipped += c.width().unwrap_or_default();
    }

    ""
}

fn status_text(response: &Response) -> String {
    let reason = response
        .status_line
        .splitn(3, ' ')
        .nth(2)
        .unwrap_or_default()
        .trim();

    if reason.is_empty() {
        response.status.to_string()
    } else {
        format!("{} {reason}", response.status)
    }
}

fn body_lines(response: &Response, kind: BodyKind) -> Vec<String> {
    let body = &response.body;

    if body.is_empty() {
        return vec!["(empty body)".into()];
    }

    if kind == BodyKind::Binary {
        let content_type = response.header("content-type").unwrap_or("unknown type");

        return vec![
            format!("binary body: {content_type}, {}", format_size(body.len())),
            "press s to save it to a file".into(),
        ];
    }

    let mut lines = match (kind, response.pretty_json()) {
        (BodyKind::Json, Some(pretty)) => pretty.lines().map(display).collect(),
        _ => text_lines(body.preview()),
    };

    if body.is_truncated() {
        lines.push(String::new());
        lines.push(format!(
            "showing the first {} of {}, press s to save the whole body",
            format_size(body.preview().len() as u64),
            format_size(body.len())
        ));
    }

    lines
}

fn text_lines(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(display)
        .collect()
}

/// longest line kept for display; there is no sideways scrolling, and a
/// multi-megabyte line would otherwise be measured on every frame
const MAX_LINE: usize = 2_000;

/// makes a line safe to draw: control characters are shown as symbols, so
/// a response cannot send escape sequences to the terminal, and very long
/// lines are cut
pub(crate) fn display(line: &str) -> String {
    let mut out = String::with_capacity(line.len().min(MAX_LINE));

    for (count, c) in line.chars().enumerate() {
        if count == MAX_LINE {
            out.push('…');
            break;
        }

        match c {
            '\t' => out.push_str("    "),
            '\0'..='\x1f' => out.push(char::from_u32(0x2400 + u32::from(c)).unwrap_or('�')),
            '\x7f' => out.push('␡'),
            c if c.is_control() => out.push('�'),
            c => out.push(c),
        }
    }

    out
}

fn timing_lines(response: &Response) -> Vec<String> {
    let t = response.timings;
    let connected = t.tls.max(t.connect);
    let mut rows = vec![
        ("DNS lookup", t.dns),
        ("TCP connect", t.connect.saturating_sub(t.dns)),
    ];

    if !t.tls.is_zero() {
        rows.push(("TLS handshake", t.tls.saturating_sub(t.connect)));
    }

    rows.push(("Waiting", t.first_byte.saturating_sub(connected)));
    rows.push(("Download", t.total.saturating_sub(t.first_byte)));

    let mut lines: Vec<String> = rows
        .into_iter()
        .map(|(label, duration)| format!("{label:<14} {:>10}", format_duration(duration)))
        .collect();
    lines.push(format!("{:<14} {:>10}", "Total", format_duration(t.total)));

    if response.redirects > 0 {
        lines.push(String::new());
        lines.push(format!(
            "{} redirect(s), ended at {}",
            response.redirects, response.effective_url
        ));
    }

    lines
}

pub(crate) fn format_duration(duration: Duration) -> String {
    let micros = duration.as_micros();

    if micros < 1_000 {
        format!("{micros} µs")
    } else if micros < 1_000_000 {
        format!("{}.{} ms", micros / 1_000, (micros % 1_000) / 100)
    } else {
        format!(
            "{}.{:02} s",
            micros / 1_000_000,
            (micros % 1_000_000) / 10_000
        )
    }
}

pub(crate) fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];

    if bytes < 1024 {
        return format!("{bytes} B");
    }

    let mut whole = bytes;
    let mut tenths = 0;
    let mut unit = 0;

    while whole >= 1024 && unit < UNITS.len() {
        tenths = (whole % 1024) * 10 / 1024;
        whole /= 1024;
        unit += 1;
    }

    format!("{whole}.{tenths} {}", UNITS[unit - 1])
}

#[cfg(test)]
mod tests {
    use super::*;
    use curst_core::http::{ResponseBody, Timings};

    fn response(status_line: &str, headers: &[(&str, &str)], body: &[u8]) -> Response {
        Response {
            status: status_line
                .split(' ')
                .nth(1)
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            status_line: status_line.into(),
            headers: headers
                .iter()
                .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
                .collect(),
            body: ResponseBody::from_bytes(body.to_vec()),
            timings: Timings {
                dns: Duration::from_micros(500),
                connect: Duration::from_millis(2),
                tls: Duration::from_millis(10),
                first_byte: Duration::from_millis(50),
                total: Duration::from_millis(1250),
            },
            effective_url: "https://h/".into(),
            redirects: 0,
            stopped: None,
        }
    }

    #[test]
    fn columns_are_skipped_by_display_width() {
        assert_eq!(skip_columns("abc", 0), "abc");
        assert_eq!(skip_columns("abc", 2), "c");
        assert_eq!(skip_columns("abc", 5), "");
        assert_eq!(skip_columns("字字x", 2), "字x");
        assert_eq!(skip_columns("字字x", 3), "x");
        assert_eq!(widest(&["字字".into(), "abc".into()]), 4);
    }

    #[test]
    fn json_is_pretty_printed() {
        let view = ResponseView::new(response(
            "HTTP/2 200",
            &[("content-type", "application/json")],
            br#"{"a":1}"#,
        ));

        assert_eq!(view.lines(ResponseTab::Body), ["{", "  \"a\": 1", "}"]);
        assert_eq!(view.width(ResponseTab::Body), 8);
        assert_eq!(view.width(ResponseTab::Headers), 30);
        assert_eq!(view.summary, "200 · 1.25 s · 7 B");
        assert_eq!(view.extension(), "json");
        assert_eq!(
            view.lines(ResponseTab::Raw),
            [
                "HTTP/2 200",
                "content-type: application/json",
                "",
                r#"{"a":1}"#
            ]
        );
    }

    #[test]
    fn binary_and_empty_bodies() {
        let view = ResponseView::new(response(
            "HTTP/1.1 200 OK",
            &[("content-type", "image/png")],
            b"\x89PNG\0",
        ));

        assert_eq!(
            view.lines(ResponseTab::Body)[0],
            "binary body: image/png, 5 B"
        );
        assert_eq!(view.lines(ResponseTab::Raw)[3], "<5 B of binary data>");
        assert_eq!(view.summary, "200 OK · 1.25 s · 5 B");
        assert_eq!(view.extension(), "bin");

        let empty = ResponseView::new(response("HTTP/1.1 204 No Content", &[], b""));

        assert_eq!(empty.lines(ResponseTab::Body), ["(empty body)"]);
        assert_eq!(empty.extension(), "txt");
    }

    #[test]
    fn timing_phases() {
        let view = ResponseView::new(response("HTTP/2 200", &[], b"x"));

        assert_eq!(
            view.lines(ResponseTab::Timing),
            [
                "DNS lookup         500 µs",
                "TCP connect        1.5 ms",
                "TLS handshake      8.0 ms",
                "Waiting           40.0 ms",
                "Download           1.20 s",
                "Total              1.25 s",
            ]
        );
    }

    #[test]
    fn control_characters_never_reach_the_terminal() {
        let view = ResponseView::new(response(
            "HTTP/1.1 200 OK\x1b[31m",
            &[("x-evil", "a\x1b]0;title\x07b")],
            b"clear\x1b[2J\x1b[H\rdone\x7f\xc2\x9b\tend",
        ));
        let all: Vec<&String> = [ResponseTab::Body, ResponseTab::Headers, ResponseTab::Raw]
            .into_iter()
            .flat_map(|tab| view.lines(tab))
            .collect();

        for line in &all {
            assert!(!line.chars().any(char::is_control), "{line:?}");
        }

        assert_eq!(
            view.lines(ResponseTab::Body),
            ["clear␛[2J␛[H␍done␡�    end"]
        );
        assert_eq!(view.lines(ResponseTab::Headers), ["x-evil: a␛]0;title␇b"]);
        assert_eq!(view.lines(ResponseTab::Raw)[0], "HTTP/1.1 200 OK␛[31m");
    }

    #[test]
    fn long_lines_are_cut() {
        let line = "x".repeat(10_000);
        let view = ResponseView::new(response("HTTP/1.1 200 OK", &[], line.as_bytes()));
        let shown = &view.lines(ResponseTab::Body)[0];

        assert_eq!(shown.chars().count(), MAX_LINE + 1);
        assert!(shown.ends_with('…'));
    }

    #[test]
    fn truncated_body_says_so() {
        let mut capped = response("HTTP/1.1 200 OK", &[("content-type", "text/plain")], b"");
        capped.body = ResponseBody::with_cap(b"line one\nline two\nline three", 9).unwrap();

        let view = ResponseView::new(capped);

        assert_eq!(
            view.lines(ResponseTab::Body),
            [
                "line one",
                "",
                "showing the first 9 B of 28 B, press s to save the whole body"
            ]
        );
    }

    #[test]
    fn sizes() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(1023), "1023 B");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(10 * 1024 * 1024), "10.0 MB");
    }
}
